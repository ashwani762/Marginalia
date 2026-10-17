# Running the node on Windows

There is no installer and no service wrapper yet. The simplest reliable way is to start the node when you log in with Task Scheduler. No code is needed.

## Once

1. Build the node (see `../README.md`) and copy `marginalia-node.exe` somewhere permanent, for example `C:\Users\<you>\AppData\Local\Marginalia\marginalia-node.exe`.
2. Put the two model files in `%APPDATA%\marginalia\models` (or choose another folder and pass `--models` below).
3. Run `marginalia-node.exe run` once from a terminal. Windows asks about the firewall. Allow it on **private networks**. If you missed the prompt, add an inbound rule in Windows Defender Firewall for UDP port 47615.
4. In another terminal run `marginalia-node.exe pair` and type the code on the phone.

## Start at login

1. Open Task Scheduler and choose **Create Task** (not Basic Task).
2. General: name it `Marginalia node`. Choose **Run only when user is logged on**. Leave **Run with highest privileges** off.
3. Triggers: **New**, Begin the task **At log on**, Specific user: you.
4. Actions: **New**, Start a program.
   - Program/script: the full path to `marginalia-node.exe`
   - Add arguments: `run` (add `--models "D:\models"` or `--cpu` if you need them)
   - Start in: the folder holding the exe
5. Conditions: untick **Start the task only if the computer is on AC power** if this is a laptop you want it on battery too. Tick **Start only if the following network connection is available: Any connection**.
6. Settings: tick **If the task fails, restart every 1 minute**, up to 3 times. Untick **Stop the task if it runs longer than** (a node should run indefinitely).
7. OK, then right-click the task and choose **Run** to test it.

The node runs in a console window. To keep it out of sight, tick **Run whether user is logged on or not** in General (Windows then asks for your password), or minimise the window.

## Notes

- `pair`, `status` and `revoke` must be run on the same computer as the running node.
- If the computer sleeps, the node is unreachable. Set **Sleep** to Never while plugged in if you want it always available.
- Files: identity and paired phones live in `%APPDATA%\marginalia-node`.
- To stop it: End the task in Task Scheduler, or close its window.
