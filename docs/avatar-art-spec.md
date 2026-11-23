# Avatar art spec — Nerea

Everything the image generation needs to produce frames the app can play. Generate **all** frames
from the same reference so the character does not drift: make the idle frame first, then feed it back
as an image reference for every other expression.

## Canvas and anchor

| Property | Value |
|---|---|
| Size | **520 × 760 px** (2× the window; the app scales down) |
| Format | PNG with **transparent** background |
| Framing | **half body**, seated, cropped mid-thigh |
| Anchor | bottom-center: the seat/hip line touches the bottom edge, horns never cropped |
| Angle | 3/4 view, looking slightly toward the viewer |
| Consistency | same camera, same scale, same lighting, same outfit in every frame |

The window is 176 × 256 logical px and sits on the corner of the focused window, so the bottom edge
is the "sitting" line. Keep the pose seated; only the face, shoulders and hands change.

## Character sheet (from her background)

- 19 years old, demon / succubus, noble origin.
- Pale skin, **dark horns**, **pointed ears**, **violet eyes**.
- Under the eyelids: violet markings that only appear when she uses her power (jealousy, anger).
- Outfit is your call, but it must be identical in all frames.

## Expressions to generate

27 frames. `fps` is how the app cycles them.

| State | Frames | FPS | Expression to draw |
|---|---|---|---|
| `idle` | 2 | 1 | neutral, calm; second frame is a blink or slight breath |
| `thinking` | 2 | 2 | eyes up and to the side, slight frown, lips pressed |
| `talking` | 3 | 8 | mouth closed / half open / open; everything else identical |
| `agree` | 2 | 2 | small nod, eyes softly closed, faint smile |
| `surprise` | 2 | 2 | eyes wide, brows up, mouth slightly open |
| `laugh` | 2 | 4 | eyes closed, mouth open in a laugh, head tilted back a little |
| `tease` | 2 | 1 | half-lidded eyes, smirk, chin slightly raised |
| `flirt` | 2 | 1 | soft half-lidded look, blush, small smile |
| `jealous` | 2 | 1 | narrowed eyes, brows down, **violet glow markings under the lids** |
| `affection` | 2 | 1 | warm smile, eyes closed, head tilted toward the viewer |
| `back` | 2 | 1 | bright eyes, leaning slightly forward, pleased |
| `goodbye` | 2 | 1 | soft smile, one hand raised a little |
| `complain` | 2 | 1 | flat brows, unimpressed stare, mouth a thin line |

Naming: `<state>-<n>.png`, for example `tease-0.png`, `tease-1.png`, `talking-2.png`.

## Prompt template

Base prompt (English works best for image models):

```
Half-body character sprite, seated pose cropped at mid-thigh, 3/4 view facing slightly
toward the viewer, anchored bottom-center, transparent background, 520x760 canvas.
Flat cel-shaded anime style, clean linework, restrained palette, soft rim light from the
upper left, no background elements, no text, no watermark.
Character: 19-year-old demon woman, pale skin, dark curved horns, long pointed ears,
violet eyes, elegant dark outfit. Calm, composed, slightly dangerous presence.
Maintain identical camera, scale, outfit and lighting across the whole set.
```

Then append one expression suffix per frame:

```
Expression: neutral and calm, eyes fully open.          (idle-0)
Expression: same as previous, eyes closed in a blink.    (idle-1)
Expression: eyes looking up and to the side, slight frown, lips pressed.  (thinking-0)
Expression: mouth closed, faint smile.                   (talking-0)
Expression: mouth half open mid-word.                    (talking-1)
Expression: mouth open as if speaking.                   (talking-2)
Expression: half-lidded eyes, smirk, chin raised slightly.  (tease-0)
Expression: narrowed eyes, brows down, violet markings glowing under the eyelids.  (jealous-0)
Expression: warm smile, eyes closed, head tilted toward the viewer.  (affection-0)
```

Negative prompt:

```
full body, standing, background, scenery, text, watermark, signature, extra characters,
different outfit, different hairstyle, cropped horns, blurry, low contrast, chibi
```

## What happens next

1. Drop the PNGs into `ui/avatar/` following the naming above.
2. I extend `ui/avatar/manifest.json` with the new states and map each phrase-bank category to one.
3. Mouth sync for `talking` is the only part that needs the 3 mouth frames; everything else is a
   2-frame loop.

If generating separate layers is possible (body / eyes / mouth), do it: it lets the app swap the
mouth while keeping the body still, which reads much better at 8 fps.
