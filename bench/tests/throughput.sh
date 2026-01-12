#!/bin/bash

PAYLOAD="1 2 4 8 16 32 64 128 256 512 1024 2048 4096 8192 16384 32768 65536 131072 262144 524288 1048576 2097152 4194304 8388608 16777216"
SEND="./target/release/thr_tcp_send"
RECV="./target/release/thr_tcp_recv"
OPTS="-i 0.5"

function reset {
    pkill -9 $(basename $RECV)
    pkill -9 $(basename $SEND)
}

trap "reset; exit -1" SIGINT SIGKILL

reset
sleep 1

for P in $PAYLOAD; do
    echo "Payload $P"

    $RECV $OPTS | awk '{ OFS =","; print "recv",'$P',$3,$5,$7}' | tail -n +2 >> /tmp/recv.log &
    sleep 1

    $SEND $OPTS -s $P | awk '{ OFS =","; print "send",'$P',$3,$5,$7}' | tail -n +2 >> /tmp/send.log &
    sleep 10.5

    reset
    sleep 1
done
