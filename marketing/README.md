# Marketing video and slides

Scripts that make the demo video and photo slides for Marginalia.

| File | What it is |
|------|------------|
| `video.py` | The 56 second, 60 fps, 1080x1920 demo video |
| `base.py` | Shared drawing helpers (fonts, text, cards, notebook, pen, phone recordings) used by `video.py` and an older version of the video (`base.py full`) |
| `slides.py` | Four 1080x1350 photo slides |
| `motion-philosophy.md` | The motion rules the video follows |

## Inputs

Screen recordings and screenshots from the phone go in `marketing/recordings/` (not committed, they show real meetings):
`m1.mp4` (live meeting), `m2.mp4` (summary), `m3.mp4` (asking), `a2.png`, `a5.png`, `n1.png`, `n3.png`, `rec3.png`.
Record them with `adb shell screenrecord` on the phone. Another folder works with `MARGINALIA_RECORDINGS=/path`.

## Run

```
python3 -m venv .venv && .venv/bin/pip install pillow numpy imageio-ffmpeg
.venv/bin/python video.py full           # writes out/marginalia-demo.mp4
.venv/bin/python video.py test 3.5 20    # single frames into out/frames
.venv/bin/python slides.py               # writes the slides into out/
```

Output goes to `marketing/out/` (not committed) or `MARGINALIA_OUT=/path`. Fonts come from the app (`app/src/main/res/font`) and Inter.
