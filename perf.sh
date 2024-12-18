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


record_perf() {
  HM_SIZE=$1
  NUM_THREADS=$2
  NUM_CLIENTS=$3
  INNER_LOOP=$4

  echo "Starting server (-s $HM_SIZE, -n $NUM_THREADS)"

  perf record -s --call-graph dwarf -o "perf_server_${NUM_CLIENTS}c_${NUM_THREADS}thr_${HM_SIZE}hms" \
    target/benchmark/server -s $HM_SIZE -n $NUM_THREADS &
  SERVER_PID=$!

  sleep 5

  echo "Starting ${NUM_CLIENTS} test clients"

  CLIENT_PIDS=()

  if (( $NUM_CLIENTS > 0 )); then
    for i in $(seq 1 $NUM_CLIENTS)
    do
      target/benchmark/client 0 $INNER_LOOP &> /dev/null &
      CLIENT_PIDS+=("$!")
    done
  fi

  echo "Recording data ..."
  sleep 15

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

record_perf 10000 2 4 10