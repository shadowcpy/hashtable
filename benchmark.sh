#!/usr/bin/env bash

set -e

trap ctrl_c INT

function ctrl_c() {
    set +e
    trap - INT
    echo "Aborting"
    killall client
    killall server
    exit 1
}

exists() {
  command -v "$1" >/dev/null 2>&1
}

if ! exists hyperfine; then
  echo "Hyperfine is not installed! Aborting"
  exit 1
fi

do_bm() {
  NAME=$1
  HM_SIZE=$2
  NUM_THREADS=$3
  NUM_CLIENTS=$4
  INNER_LOOP=$5
  OUTER_LOOP=$6
  WARMUP_RUNS=${7:-40}

  echo "-- Benchmark $NAME (server -s $HM_SIZE -n $NUM_THREADS) ($NUM_CLIENTS bg clients, iLoop iterations $INNER_LOOP) --"

  target/benchmark/server -s $HM_SIZE -n $NUM_THREADS &> /dev/null &
  SERVER_PID=$!

  CLIENT_PIDS=()

  if (( $NUM_CLIENTS > 0 )); then
    for i in $(seq 1 $NUM_CLIENTS)
    do
      target/benchmark/client 0 $INNER_LOOP &> /dev/null &
      CLIENT_PIDS+=("$!")
    done
  fi

  sleep 1

  hyperfine -N --warmup $WARMUP_RUNS "target/benchmark/client $OUTER_LOOP $INNER_LOOP" --export-csv "analysis/benchmarks/$NAME.csv"
  if (( $NUM_CLIENTS > 0 )); then
    for pid in $CLIENT_PIDS
    do
      kill -2 $pid
      wait $pid
    done
  fi

  kill -2 $SERVER_PID
  wait $SERVER_PID
}

do_bm "AloneSingleThread" 10000 1 0 10 100
do_bm "TwoSingleThread" 10000 1 1 10 100

do_bm "AloneMultiThread" 10000 4 0 10 100
do_bm "TwoMultiThread" 10000 4 1 10 100

do_bm "ManyClientsST" 10000 1 16 10 100 2
do_bm "ManyClientsMT" 10000 4 16 10 100 2

do_bm "SCManyThreads" 10000 32 0 10 100

target/benchmark/evaluator
