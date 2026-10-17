# Working on this repository

On Windows (working only on the compute node and its tray icon): read `docs/WINDOWS-PLAN.md` FIRST and nothing else until it
sends you somewhere. It has the four commands to start, the list of likely Windows problems with the file for each, and what not to
re-check. Run `.\scripts\windows\verify.ps1`, fix the first failing line, use `.\scripts\windows\dev.ps1` to rebuild and
restart. After a `git pull` run `.\scripts\windows\setup.ps1 -Quick`.

Everywhere else read `docs/ARCHITECTURE.md` and `docs/DEVELOPMENT.md` first.

Rules that matter: user-visible text uses plain wording with no semicolons and no em dashes, the feature is called "compute node"
(never "home" or "computer"), commit messages are `[area] subject`, and models, databases and tokens are never committed.
