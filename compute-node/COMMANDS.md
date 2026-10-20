# Compute node commands

Every command of `marginalia-node` (the node), `marginalia-node-tray` (the tray icon) and `marginalia-node-speakers` (the
speaker helper), with examples. On Windows the programs end in `.exe` and work the same.

Two options work with every node command:

| Option | What it does |
|--------|--------------|
| `--state-dir DIR` | Where the node keeps its identity, its paired phones and its settings. Defaults to `~/.config/marginalia-node` (Linux) or `%APPDATA%\marginalia-node` (Windows). |
| `--admin-port PORT` | The local control port (this machine only). Default 47616. Use another one to run a second node for testing. |

Settings saved by `chat-model` and `run-on` are small files in the state folder and are read when the node starts, so restart
the node after changing them (the tray's Settings page does that for you). The list of model files and the memory
estimates are in `models.toml`, see [The models file](#the-models-file).

## Running the node

### `run`

Starts the node. It loads the models, announces itself on the network and serves phones until it is stopped.

```
marginalia-node run
marginalia-node run --name "Kitchen node"
marginalia-node run --models /data/models --lock
marginalia-node run --run-on cpu
marginalia-node run --port 47725 --admin-port 47726 --state-dir /tmp/test-node --name test-node
```

| Option | What it does |
|--------|--------------|
| `--port PORT` | The port phones connect to (default 47615). |
| `--name NAME` | The name phones see. Defaults to this machine's name. |
| `--models DIR` | The models folder. Defaults to `~/.local/share/marginalia/models` (Linux) or `%APPDATA%\marginalia\models` (Windows). |
| `--chat e2b\|e4b` | Which chat model to load for this run only. Without it the choice from `chat-model` is used. |
| `--chat-model FILE`, `--embed-model FILE`, `--speech-model FILE` | Use these exact files instead of the ones in the models folder. |
| `--no-speech` | Do not load the speech model, so this node does not transcribe recordings. |
| `--run-on auto\|gpu\|cpu\|mixed` | Where the models run, for this run only. Without it the choice from `run-on` is used, then `gpu`. |
| `--cpu` | The same as `--run-on cpu`. |
| `--ctx TOKENS` | The longest prompt plus answer the chat model handles (default 16384). Lower it to share a small graphics card. |
| `--lock` | Start locked: a new phone has to be allowed with `approve` or pair with a code. |
| `--window-max-seconds N` | Audio this short or shorter counts as a live window and runs next to a long recording (default 120). |

### `stop`, `pause`, `resume`

```
marginalia-node pause     # turn new jobs away, the models stay loaded, running jobs finish
marginalia-node resume    # take jobs again
marginalia-node stop      # finish running jobs (up to 10 seconds), leave the network, exit
```

Phones see a paused node as yellow and a stopped one as red.

### `status`, `stats`, `logs`

```
marginalia-node status            # what the node serves, where each model runs, phones, peers (JSON)
marginalia-node stats             # jobs, speed and errors so far. Never any content
marginalia-node logs              # the last 40 lines of the node's own log
marginalia-node logs --lines 200
marginalia-node logs --follow     # keep showing new lines (Ctrl-C to stop)
```

The log is `logs/node.log` in the state folder, 5 files of 5 MB at most. It holds events and numbers only, never what a phone sent.
`status` has a `placement` part that says where each model ended up, for example
`"chat": {"on": "mixed", "gpu_layers": 16, "layers": 35}`.

## Choosing models and where they run

### `chat-model`

```
marginalia-node chat-model        # show the choice
marginalia-node chat-model e4b    # the larger model, answers better, needs about 2 GB more memory
marginalia-node chat-model e2b    # the smaller, faster model (the default)
```

If the file is missing: `marginalia-node models download --chat-model e4b`.

### `run-on`

Where each model runs. With no choice it shows the current setting and what the node would pick on this machine right now.

```
marginalia-node run-on                      # show the setting and a suggestion for this machine
marginalia-node run-on auto                 # the node decides each model from the free graphics memory
marginalia-node run-on gpu                  # everything on the graphics card (the default)
marginalia-node run-on cpu                  # everything on the processor, the graphics card stays free
marginalia-node run-on mixed                # the node splits the chat and search models to fit, speech and photos on the card
marginalia-node run-on mixed --chat 60 --search 100 --speech gpu --photos cpu
marginalia-node run-on gpu --speakers gpu   # also tell speakers apart on the graphics card
```

| Choice | What happens |
|--------|--------------|
| `auto` | When the node starts it reads the free graphics memory and places the models: speech first, then photos, then the search model, then as much of the chat model as fits. Everything that does not fit runs on the processor. |
| `gpu` | Everything on the graphics card. A model that does not fit falls back to fewer layers, then to the processor. |
| `cpu` | Everything on the processor. Slower. Good when the graphics card is busy with something else. |
| `mixed` | Each model where you say. Shares you leave out are worked out from the free graphics memory, so `mixed` on its own is close to `auto`. The tray calls this Custom. |

| Option (mixed only, except `--speakers`) | What it does |
|--------|--------------|
| `--chat PERCENT` | The share of the chat model's layers on the graphics card, 0 to 100. 0 is the processor only, 100 the card only. |
| `--search PERCENT` | The same for the search model. |
| `--speech gpu\|cpu` | Where the speech model runs. whisper.cpp cannot split a model, so it is one or the other. |
| `--photos gpu\|cpu` | Where photos are read (the vision projector). Also one or the other. |
| `--speakers gpu\|cpu` | Where speakers are told apart. Works with every choice. `gpu` needs the speaker helper built with the GPU version of sherpa-onnx and cuDNN, otherwise it runs on the processor and `status` says so. Left out, the saved choice is kept. |
| `--models DIR` | The models folder used for the suggestion. |

Measured on an RTX 4060 (8 GB) with another program using 3.5 GB, E2B chat model:

| Setting | Chat model | Chat speed | Speech |
|---------|-----------|------------|--------|
| `cpu` | processor | 22.6 tokens a second | 2.5 times faster than real time |
| `mixed` (worked out: 45 percent) | 16 of 35 layers on the card | 41 tokens a second | 88 times faster than real time |
| `auto` (4.3 GB free) | 14 of 35 layers on the card, speech, search and photos on the card | 15.8 tokens a second | 64 times faster than real time |
| `mixed --chat 30 --search 0 --speech cpu --photos cpu` | 11 of 35 layers on the card | 34.9 tokens a second | 2.5 times faster than real time |

The saved setting is one line in the state folder (`run-on`), for example `mixed chat=60 search=0 speech=cpu photos=gpu speakers=gpu`.

### `models list`, `models download`

```
marginalia-node models list                          # each model file, whether it is there, its size and the folder
marginalia-node models download                      # chat (E2B), search and speech, about 3.7 GB
marginalia-node models download --chat-model e4b     # the larger chat model instead
marginalia-node models download --only vision        # photos (about 0.56 GB, follows --chat-model)
marginalia-node models download --only speakers      # telling speakers apart (about 0.03 GB)
marginalia-node models download --only speech
marginalia-node models download --skip-speech
marginalia-node models download --dir /data/models
```

A stopped download continues where it stopped. If a repository asks for a login, set the `HF_TOKEN` environment variable. It
is only sent to Hugging Face and never printed.

```
marginalia-node models list --json                                         # the same list as JSON, for the tray
MARGINALIA_PROGRESS_FILE=/tmp/dl.json marginalia-node models download --only speakers
```

With `MARGINALIA_PROGRESS_FILE` the download keeps one line of JSON in that file: the file being fetched, bytes done and
expected, speed, and at the end `done` or the reason it failed. The tray's Settings page uses it.

## The models file

`models.toml` lists every model file the node can use (its role, Hugging Face repository and file name) and the memory
estimates that `run-on auto` and `mixed` use to decide what fits. The node reads the first one it finds:

1. the file named by the `MARGINALIA_MODELS_FILE` environment variable
2. `models.toml` in the state folder
3. `models.toml` next to the `marginalia-node` program
4. the one built into the program (`compute-node/models.toml` in the repository)

A model left out of your file comes from the built-in one, so a small file with only what you change is enough. For example,
to try another search model without rebuilding:

```toml
[models.search]
role = "search"
repo = "someone/other-embedding-GGUF"
file = "other-embedding-Q8_0.gguf"
```

`marginalia-node models list` shows which file is in use. Restart the node after editing it.

## Phones

```
marginalia-node pair              # a one-time code (5 minutes) to type on the phone, and the node's address
marginalia-node lock              # new phones must be allowed first
marginalia-node unlock            # any phone on the network may ask (the default)
marginalia-node approve           # let the waiting phone in
marginalia-node approve 3f2a      # the one with this id when several wait
marginalia-node deny              # turn the waiting phone down
marginalia-node block "Old phone" # turn a phone away for good
marginalia-node unblock "Old phone"
marginalia-node revoke "Pixel"    # forget a paired phone (id from status, or its name)
```

On the phone: Settings, Compute nodes, Add a compute node, then type the code from `pair`.

## Several compute nodes

```
marginalia-node pair                                   # on the node that is already there
marginalia-node join 192.168.1.20:47615 123456         # on the new one, with that node's address and code
marginalia-node join "Kitchen node" 123456             # or by its name when it is seen on the network
marginalia-node peers                                  # the nodes this one has joined and whether they answer
marginalia-node unjoin "Kitchen node"
```

Phones paired with one node then work with all of them, and a busy node sends jobs to a free one.

## Checking a node like a phone

`probe` connects the way a phone does. Useful after an install or a change.

```
marginalia-node probe 127.0.0.1:47615 --code 123456                  # pair, then try search and chat
marginalia-node probe 127.0.0.1:47615 --knock                        # ask to be let in without a code
marginalia-node probe 127.0.0.1:47615 --only chat --long             # one long answer
marginalia-node probe 127.0.0.1:47615 --transcribe talk.wav --language en
marginalia-node probe 127.0.0.1:47615 --transcribe clip.wav --window --prompt "Earlier we said"
marginalia-node probe 127.0.0.1:47615 --speakers meeting.wav --people 3
```

Audio for `--transcribe` and `--speakers` is 16 kHz mono 16 bit `.wav` or raw `.pcm`.

## The tray icon

```
marginalia-node-tray                       # the icon in the top bar or the taskbar
marginalia-node-tray --print               # print the menu as text once and exit, to check the setup
marginalia-node-tray --panel               # open only the details panel
marginalia-node-tray --panel --settings    # open the panel on its Settings page
marginalia-node-tray --admin-port 47726    # watch a node on another control port
```

The menu has Pause or Resume, Show details, Settings, Stop node (or Start node) and Quit tray. A second tray for the same
node does not start.

The panel has two pages:

- **Overview**: what the node is doing, phones, peers, live graphs, and the Pause, pairing, Lock and Stop buttons.
- **Settings**: where each model runs now, the chat model (E2B or E4B), where the models run (Auto, Graphics card,
  Processor or Custom, where Custom gives a share for the chat and search models and a side for speech and photos), where
  speakers are told apart, the models with a Get button for each missing one and the download in progress, and the log
  folder. Save and restart writes the settings with `chat-model` and `run-on` into the node's own state folder and
  restarts that node.

## The speaker helper

`marginalia-node-speakers` is started by the node for each job. It is a separate program because ONNX Runtime and llama.cpp
in one program crash at start. Run by hand only to test it:

```
marginalia-node-speakers --segmentation pyannote-segmentation-3-0.onnx --voices wespeaker-resnet34.onnx --audio meeting.pcm --shift 0.5
marginalia-node-speakers ... --people 2 --threshold 0.6 --threads 8
marginalia-node-speakers ... --provider cuda
```

It prints one line of JSON with the spans. `--provider cuda` needs the GPU build (see README.md, Speakers on the graphics
card). The audio file is raw 16 kHz mono 16 bit PCM.
