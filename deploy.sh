#!/bin/bash

set -e

cargo build --release

# Send SIGTERM to any running timings-app so it writes pending timings
# and exits gracefully before we replace the binary
if pkill -SIGTERM timings-app 2>/dev/null; then
    echo "Sent SIGTERM to running timings-app, waiting for graceful shutdown..."
    # Wait up to 5 seconds for the process to exit
    for i in {1..50}; do
        if ! pgrep timings-app >/dev/null 2>&1; then
            echo "timings-app exited gracefully"
            break
        fi
        sleep 0.1
    done
fi

cp --backup ~/.config/timings/timings.db ~/.config/timings/timings.db.bak
cp --backup target/release/timings-app ~/.config/timings/timings-app

# Restart the app in the user's systemd session (same as a .desktop file launch)
systemd-run --user --slice=app.slice --collect ~/.config/timings/timings-app
echo "Launched timings-app via systemd-run. View logs: journalctl --user -f -u"
