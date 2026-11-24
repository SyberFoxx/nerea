//! Turning a file into text, whatever the file is.
//!
//! Text and code are read as they are. PDF, Word, OpenDocument, EPUB and HTML
//! need their container taken apart first. Anything with a NUL byte in it is
//! not text, and saying so is better than dumping a megabyte of noise.

use std::fs;
use std::io::Read;
use std::path::Path;

/// Nothing bigger is opened: a 2 GB file would eat the whole context budget.
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// A wrapped XML part is not allowed to expand past this.
const MAX_PART_BYTES: u64 = 48 * 1024 * 1024;
const HEAD: usize = 8_192;

/// Search skips the formats that need a real parser: they are slow and noisy.
pub fn is_plain_text(path: &Path) -> bool {
    !matches!(extension(path).as_str(), "pdf" | "docx" | "odt" | "epub")
}

pub fn extract(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path)
        .map_err(|err| format!("no puedo abrir {}: {err}", path.display()))?;
    if meta.is_dir() {
        return Err("es una carpeta, no un archivo".to_owned());
    }
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "pesa {} MB y no lo abro entero",
            meta.len() / 1_048_576
        ));
    }

    match extension(path).as_str() {
        "pdf" => {
            let bytes = fs::read(path).map_err(|err| err.to_string())?;
            let text = pdf_extract::extract_text_from_mem(&bytes)
                .map_err(|_| "no pude extraer el texto del PDF: puede ser un escaneo sin capa de texto".to_owned())?;
            non_empty(tidy(&text), path, "el PDF no tiene texto extraíble")
        }
        "docx" => zip_part(path, "word/document.xml"),
        "odt" => zip_part(path, "content.xml"),
        "epub" => epub(path),
        "html" | "htm" => {
            let bytes = fs::read(path).map_err(|err| err.to_string())?;
            non_empty(tidy(&strip_markup(&read_text(&bytes))), path, "el HTML está vacío")
        }
        _ => {
            let bytes = fs::read(path).map_err(|err| err.to_string())?;
            if looks_binary(&bytes) {
                return Err(
                    "parece un archivo binario: no tiene texto legible (imágenes, audio y vídeo no los puedo leer)"
                        .to_owned(),
                );
            }
            non_empty(read_text(&bytes), path, "el archivo está vacío")
        }
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(HEAD)].contains(&0)
}

fn read_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    text.trim_end().to_owned()
}

fn non_empty(text: String, path: &Path, message: &str) -> Result<String, String> {
    let text = tidy(&text);
    if text.trim().is_empty() {
        return Err(format!("{}: {message}", path.display()));
    }
    Ok(text)
}

/// Collapses the blank runs markup leaves behind.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blanks = 0usize;

    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
        out.push('\n');
    }

    out.trim().to_owned()
}

fn zip_part(path: &Path, name: &str) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|err| err.to_string())?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|_| "no pude abrir el documento: ¿está dañado?".to_owned())?;

    let mut entry = archive
        .by_name(name)
        .map_err(|_| "el documento no tiene el contenido que esperaba".to_owned())?;
    if entry.size() > MAX_PART_BYTES {
        return Err("el documento es demasiado grande para leerlo de una vez".to_owned());
    }

    let mut xml = String::new();
    entry
        .read_to_string(&mut xml)
        .map_err(|err| format!("no pude leer el contenido del documento: {err}"))?;

    non_empty(tidy(&strip_markup(&xml)), path, "el documento no tiene texto")
}

fn epub(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|err| err.to_string())?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|_| "no pude abrir el EPUB: ¿está dañado?".to_owned())?;

    let names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(|name| name.to_string())
        .collect();
    let mut text = String::new();

    for name in names {
        let lower = name.to_lowercase();
        if !(lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")) {
            continue;
        }

        let Ok(mut entry) = archive.by_name(&name) else { continue };
        if entry.size() > MAX_PART_BYTES {
            continue;
        }
        let mut chapter = String::new();
        if entry.read_to_string(&mut chapter).is_ok() {
            text.push_str(&strip_markup(&chapter));
            text.push_str("\n\n");
        }
    }

    non_empty(tidy(&text), path, "el EPUB no tiene texto")
}

/// Keeps the words, drops the tags, and turns paragraph ends into newlines.
fn strip_markup(xml: &str) -> String {
    let text = xml
        .replace("</w:p>", "\n")
        .replace("</text:p>", "\n")
        .replace("</text:h>", "\n")
        .replace("</p>", "\n")
        .replace("<br/>", "\n")
        .replace("<br>", "\n")
        .replace("<w:br/>", "\n")
        .replace("</w:tr>", "\n")
        .replace("</text:tab/>", "\t");

    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for character in text.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(character),
            _ => {}
        }
    }

    decode_entities(&out)
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nerea-extract-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn plain_text_comes_back_as_is() {
        let dir = scratch("text");
        let file = dir.join("notes.md");
        fs::write(&file, "# Hola\r\n\r\nmundo\r\n").expect("write");

        let text = extract(&file).expect("extract");
        assert_eq!(text, "# Hola\n\nmundo");
    }

    #[test]
    fn a_binary_file_is_refused_with_a_reason() {
        let dir = scratch("binary");
        let file = dir.join("avatar.png");
        fs::write(&file, [0x89, b'P', b'N', b'G', 0, 0, 1]).expect("write");

        let err = extract(&file).expect_err("must refuse");
        assert!(err.contains("binario"), "{err}");
    }

    #[test]
    fn a_word_document_gives_up_its_paragraphs() {
        let dir = scratch("docx");
        let file = dir.join("lore.docx");

        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("word/document.xml", options).expect("start");
            zip.write_all(
                br#"<w:document><w:body><w:p><w:r><w:t>Nerea Lyricaela</w:t></w:r></w:p><w:p><w:r><w:t>Lioner Eingerhart</w:t></w:r></w:p></w:body></w:document>"#,
            )
            .expect("write xml");
            zip.finish().expect("finish");
        }
        fs::write(&file, buffer.into_inner()).expect("write docx");

        let text = extract(&file).expect("extract");
        assert!(text.contains("Nerea Lyricaela"), "{text}");
        assert!(text.contains("Lioner Eingerhart"), "{text}");
        assert!(text.contains('\n'), "paragraphs must be separate lines: {text}");
    }

    #[test]
    fn an_odt_document_gives_up_its_paragraphs() {
        let dir = scratch("odt");
        let file = dir.join("lore.odt");

        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("content.xml", options).expect("start");
            zip.write_all("<office><text:p>Princesa Sombría</text:p></office>".as_bytes())
                .expect("write xml");
            zip.finish().expect("finish");
        }
        fs::write(&file, buffer.into_inner()).expect("write odt");

        let text = extract(&file).expect("extract");
        assert!(text.contains("Princesa Sombría"), "{text}");
    }

    #[test]
    fn html_loses_its_tags_and_keeps_its_text() {
        let dir = scratch("html");
        let file = dir.join("page.html");
        fs::write(&file, "<h1>Hola</h1><p>mundo &amp; compañía</p>").expect("write");

        let text = extract(&file).expect("extract");
        assert!(text.contains("Hola"), "{text}");
        assert!(text.contains("mundo & compañía"), "{text}");
        assert!(!text.contains('<'), "{text}");
    }

    #[test]
    fn a_broken_pdf_fails_gracefully() {
        let dir = scratch("pdf");
        let file = dir.join("broken.pdf");
        fs::write(&file, b"not really a pdf").expect("write");

        let err = extract(&file).expect_err("must refuse");
        assert!(err.contains("PDF"), "{err}");
    }

    #[test]
    fn markup_entities_are_decoded() {
        assert_eq!(decode_entities("a &lt;b&gt; &amp; c"), "a <b> & c");
    }
}
