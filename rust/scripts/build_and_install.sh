#!/bin/bash

echo "Getting sudo"
sudo -v

echo "Building via cargo"
cargo build --release

echo "Move bin"
sudo mv target/release/prismarine_launcher /usr/bin/
