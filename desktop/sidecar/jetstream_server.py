"""Entry point for the JetStream server bundled inside the desktop app (PyInstaller sidecar)."""

import multiprocessing
import os
import sys
import threading
import time


def _exit_when_parent_exits(parent_pid: int) -> None:
    """Stop the server if the desktop app goes away (crash, kill, or quit)."""
    import psutil

    while True:
        time.sleep(2)
        if not psutil.pid_exists(parent_pid):
            os._exit(0)


def main() -> int:
    multiprocessing.freeze_support()

    parent_pid = os.environ.get("JETSTREAM_DESKTOP_PARENT_PID")
    if parent_pid and parent_pid.isdigit():
        threading.Thread(target=_exit_when_parent_exits, args=(int(parent_pid),), daemon=True).start()

    # The app may be installed in a read-only folder; run (and write job logs) from the user data dir.
    from jetstream.config import user_data_dir

    data_dir = user_data_dir()
    data_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(data_dir)
    os.environ.setdefault("AUTO_OPEN_BROWSER", "false")
    os.environ["BROWSER_OPENED"] = "true"

    from jetstream.cli import main as cli_main

    return cli_main()


if __name__ == "__main__":
    sys.exit(main())
