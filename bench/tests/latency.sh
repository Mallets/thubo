#!/bin/bash

PAYLOAD="1 2 4 8 16 32 64 128 256 512 1024 2048 4096 8192 16384 32768 65536 131072 262144 524288 1048576 2097152 4194304 8388608 16777216"
PING="./target/release/ping"
PONG="./target/release/pong"
OPTS="-t -w 1.0 -n 100"

function reset {
    pkill -9 $(basename $PONG)
    pkill -9 $(basename $PING)
}

trap "reset; exit -1" SIGINT SIGKILL

reset
sleep 1

for P in $PAYLOAD; do
    echo "Payload $P"

    $PONG &
    sleep 1

    $PING $OPTS -s $P >> /tmp/ping.log

    reset
    sleep 1
done
