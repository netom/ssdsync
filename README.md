# SSDSync

**WARNING: this is experimental software. It can very
well destroy all your data.**

SSDSync tries to copy data between files or block devices
using as few writes as possible. This is to protect
SSDs from wear.

SSDSync aims to replace simple "dd" style data transfer
solutions. It reads both source and target, and compares
blocks. Only differing blocks are written to the target.

## Install

First, install the Rust toolchain from https://rustup.rs/.

Then you can install SSDSync directly from github with cargo:

```
cargo install  --git https://github.com/netom/ssdsync.git
```
## Run

```
$ ssdsync --help
Usage: ssdsync [OPTIONS] <SOURCE> <TARGET>

Arguments:
  <SOURCE>  Source file or device
  <TARGET>  Target file or device

Options:
  -b, --block-size <BLOCK_SIZE>  Size of blocks in bytes to read/write at once [default: 16384]
  -h, --help                     Print help
  -V, --version                  Print version

```

SSDSync runs quite fast, but it can benefit from pinning onto a CPU core:

```
taskset -c 0 ssdsync ...
```

## Build

A binary with debugging enabled can be built with cargo:

```
cargo build
```

You can install a local release build by:

```
cargo install --path .
```
