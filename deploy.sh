#!/bin/bash

set -e

cargo build --release
cp --backup ~/.config/timings/timings.db ~/.config/timings/timings.db.bak
cp --backup target/release/timings-app ~/.config/timings/timings-app