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

## Tray icon

`marginalia-node-tray.exe` puts a small status icon in the system tray (the area by the clock, maybe behind the `^` arrow). A left click opens a small details panel (jobs, phones, speed, models, activity, addresses, with buttons to pause, lock, show a pairing code, allow or block phones and stop the node). A right click shows a short menu: status, Pause or Resume, Show details, Stop node and Quit tray. When the node is not running the panel and the menu offer "Start node".

Note: the Windows version of the tray and the panel are written but have not been compiled or run yet. The panel uses OpenGL, so a machine with very old graphics drivers may not show it. Please report anything that does not work.

1. Build it with `cargo build --release -p marginalia-node-tray` and copy `marginalia-node-tray.exe` into the same folder as `marginalia-node.exe`. "Start node" looks for `marginalia-node.exe` next to the tray, then in PATH.
2. Start it at login with the Startup folder: press Win+R, type `shell:startup`, and put a shortcut to `marginalia-node-tray.exe` in the folder that opens. (Task Scheduler works too: another task with trigger **At log on**, Start a program, no arguments.)
3. If you gave the node a different `--admin-port`, give the tray the same: add `--admin-port 47620` to the shortcut's Target, after the path.

The tray only talks to the node on this computer (127.0.0.1) and only receives counts and names. The panel is a second process (`marginalia-node-tray.exe --panel`) that the tray starts on a click, so keep it in the same folder. Errors such as "Could not pause the node" appear in a message box.

When you pick "Stop node" the node finishes running jobs (up to 10 seconds) and exits cleanly, so Task Scheduler's "restart if the task fails" does not bring it back. Use "Start node" in the tray, or run the task again.

## Notes

- `pair`, `status`, `pause`, `resume`, `stop` and `revoke` must be run on the same computer as the running node.
- If the computer sleeps, the node is unreachable. Set **Sleep** to Never while plugged in if you want it always available.
- Files: identity and paired phones live in `%APPDATA%\marginalia-node`.
- To stop it: `marginalia-node.exe stop`, or "Stop node" in the tray, or End the task in Task Scheduler, or close its window.
