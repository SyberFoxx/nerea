# Avatar art spec — Nerea (human, one frame per reaction)

One static frame per reaction. No animation loops, no mouth cycles: each state is a single PNG the
app shows while that state is active.

## Workflow

1. Take the reference image of Nerea that already exists.
2. Use it as the **image reference** for every generation, so the face, hair, outfit and palette
   never drift.
3. Ask the model to **change only the expression and the body language**, never the character.
4. Export each result as a transparent PNG at the same size and crop.

**Human version**: no horns, no pointed ears, no demon markings, no fangs. Normal ears and skin.
Keep her face, hair, eye colour and outfit from the reference.

## Canvas and anchor

| Property | Value |
|---|---|
| Size | **520 × 760 px** |
| Format | PNG, transparent background |
| Framing | **half body**, seated, cropped mid-thigh — identical crop in every frame |
| Anchor | bottom-center: hip/seat line touches the bottom edge |
| View | 3/4, facing slightly toward the viewer |
| Consistency | same camera, scale, lighting, outfit and hair in all frames |

The window is 176 × 256 logical px and sits on the corner of the focused window, so the bottom edge
is the line where she "sits". Everything above the hips is what you will actually see.

## The 15 frames

| File | When the app shows it | Expression and body |
|---|---|---|
| `idle.png` | nothing happening | neutral, calm, eyes fully open, lips relaxed, shoulders down |
| `hello.png` | app opens, first line of the day | eyes bright, small warm smile, head tilted a little toward the viewer |
| `thinking.png` | while she is generating a reply | eyes up and to her right, brows slightly drawn, lips pressed, one shoulder raised |
| `talking.png` | while her voice plays | mouth open mid-word, brows relaxed, a small gesture with one hand |
| `agree.png` | you confirmed something | gentle nod: chin slightly down, eyes softly closed, faint smile |
| `surprise.png` | something unexpected | eyes wide, brows high, mouth slightly open, slight lean back |
| `laugh.png` | you made a joke | eyes closed in a smile, mouth open, head tilted back a little, hand near the mouth |
| `tease.png` | you made a mistake | half-lidded eyes, one brow up, crooked smirk, chin raised |
| `flirt.png` | affectionate moment | soft half-lidded look, light blush, small smile, head tilted, shoulder toward the viewer |
| `jealous.png` | another woman is mentioned | eyes narrowed, brows down, smile gone, cold flat stare, chin lowered |
| `affection.png` | you thanked her or did well | warm smile, eyes closed, head tilted toward the viewer, relaxed shoulders |
| `back.png` | you returned after being away | leaning slightly forward, eyes bright, pleased smile |
| `goodbye.png` | you close the app | soft smile, one hand raised a little, eyes slightly down |
| `complain.png` | the API failed | flat brows, unimpressed stare, mouth a thin line, head slightly to the side |
| `sleepy.png` | very late at night | heavy lids, small yawn suppressed, head resting on one hand |

## Prompt template

Prefix — identical for every frame:

```
Using the provided reference image, keep the exact same character: same face, same hair,
same eye colour, same outfit, same palette, same art style. Human woman, normal human ears,
no horns, no pointed ears, no demon traits, no fangs.

Half-body character sprite, seated pose cropped at mid-thigh, 3/4 view facing slightly
toward the viewer, anchored bottom-center, transparent background, 520x760 canvas.
Clean cel-shaded anime style, soft rim light from the upper left, no background,
no text, no watermark. Identical camera, scale, lighting and outfit as the reference.
```

Then one expression line per frame:

```
Expression: neutral and calm, eyes fully open, lips relaxed.                    (idle)
Expression: bright welcome, small warm smile, head tilted toward the viewer.    (hello)
Expression: eyes looking up and to her right, brows slightly drawn, lips pressed, one shoulder raised.  (thinking)
Expression: mouth open mid-word as if speaking, relaxed brows, one hand gesturing.  (talking)
Expression: gentle nod, chin slightly down, eyes softly closed, faint smile.    (agree)
Expression: eyes wide, brows high, mouth slightly open, leaning back a little.  (surprise)
Expression: eyes closed in a smile, mouth open laughing, head tilted back, hand near the mouth.  (laugh)
Expression: half-lidded eyes, one brow raised, crooked smirk, chin raised.      (tease)
Expression: soft half-lidded look, light blush, small smile, head tilted, shoulder toward the viewer.  (flirt)
Expression: eyes narrowed, brows down, no smile, cold flat stare, chin lowered. (jealous)
Expression: warm smile, eyes closed, head tilted toward the viewer.             (affection)
Expression: leaning slightly forward, bright eyes, pleased smile.               (back)
Expression: soft smile, one hand raised a little, eyes slightly down.           (goodbye)
Expression: flat brows, unimpressed stare, mouth a thin line, head slightly turned.  (complain)
Expression: heavy eyelids, small suppressed yawn, head resting on one hand.     (sleepy)
```

Negative prompt, every time:

```
horns, pointed ears, demon, fangs, tail, wings, glowing markings, full body, standing,
background, scenery, text, watermark, signature, extra characters, different outfit,
different hairstyle, different face, cropped head, blurry, chibi
```

## Checks before handing them over

- Transparent background, no white pixels at the edges.
- The head sits at the same height in every frame; overlay two frames to confirm.
- The bottom edge is the same in every frame (that is the seat line).
- No horns, no pointed ears, nothing demonic left anywhere.

Drop the files in `ui/avatar/` with these exact names. I extend `manifest.json`, map each
phrase-bank category to its frame, and the reactions start firing.
