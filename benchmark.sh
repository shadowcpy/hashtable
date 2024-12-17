#!/usr/bin/env bash

exists() {
  command -v "$1" >/dev/null 2>&1
}

if ! exists hyperfine; then
  echo "Hyperfine is not installed! Aborting"
  exit 1
fi

target/benchmark/server -s 1000 -n 1 &> /dev/null & 
SERVER_PID=$!


hyperfine  -N --warmup 10 -L num_inner 10,100,1000 "target/benchmark/client 10 {num_inner}"
kill -2 $SERVER_PID
