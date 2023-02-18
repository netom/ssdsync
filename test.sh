#!/bin/bash

# Compare files, succeed if same
assert_eq () {
    if [ "$(sha1sum < $1)" == "$(sha1sum < $2)" ]; then
        echo "OK: $1 == $2"
    else
        echo "FAILED: $1 != $2"
        exit 1
    fi
}

# Compare files, succeed if different
assert_ne () {
    if [ "$(sha1sum < $1)" != "$(sha1sum < $2)" ]; then
        echo "OK: $1 != $2"
    else
        echo "FAILED: $1 == $2"
        exit 1
    fi
}

cargo build

SSDSYNC=./target/debug/ssdsync

TESTPATH=/dev/shm/ssdsync-test

mkdir -p $TESTPATH

F1=$TESTPATH/f1
F2=$TESTPATH/f2

# Two identical files, all zeroes

dd if=/dev/zero of=$F1 bs=1000 count=1
dd if=/dev/zero of=$F2 bs=1000 count=1

assert_eq $F1 $F2

$SSDSYNC $F1 $F2

assert_eq $F1 $F2

# Two different files, all zeroes and random

dd if=/dev/urandom of=$F1 bs=1000 count=1
dd if=/dev/zero of=$F2 bs=1000 count=1

assert_ne $F1 $F2

$SSDSYNC -b 17 $F1 $F2

assert_eq $F1 $F2

rm -rf $TESTPATH
