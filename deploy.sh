#!/bin/bash

set -e

cargo build --release
cp target/release/timings-app ~/.config/timings/