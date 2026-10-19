# Marketing video and slides

Scripts that make the LinkedIn video and photo slides for Marginalia, plus the post text.

| File | What it is |
|------|------------|
| `video.py` | The 56 second, 60 fps, 1080x1920 demo video (calm mask reveals, phone to close-up morphs) |
| `base.py` | Shared drawing helpers (text, cards, notebook, phone) used by `video.py` |
| `slides.py` | Four 1080x1350 photo slides |
| `motion-philosophy.md` | The motion rules |
| `linkedin-post.txt` | The post |

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
