#!/bin/bash
# run.sh <label> <port> <tokenizer dir> <bench extra args (quoted)> -- <server command…>
# Starts the server, times launch → /health, samples the GPU, runs bench.py, stops the server.
set -u
label=$1 port=$2 tokdir=$3 bargs=$4; shift 5
OUT=/w/out; mkdir -p $OUT
log=$OUT/$label.server.log
nvidia-smi --query-gpu=memory.used,utilization.gpu --format=csv,noheader,nounits -lms 500 > $OUT/$label.gpu.csv &
smi=$!
t0=$(date +%s.%N)
setsid "$@" > "$log" 2>&1 &
srv=$!
ok=0
for _ in $(seq 1 1800); do
  if curl -sf "http://127.0.0.1:$port/health" > /dev/null 2>&1; then ok=1; break; fi
  kill -0 $srv 2> /dev/null || break
  sleep 1
done
t1=$(date +%s.%N)
load=$(python3 -c "print(round($t1-$t0,1))")
if [ $ok != 1 ]; then echo "{\"label\":\"$label\",\"error\":\"server never healthy\",\"load_s\":$load}" | tee $OUT/$label.load.json; tail -40 "$log"; kill -- -$srv 2> /dev/null; kill $smi; exit 1; fi
idle=$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits | head -1)
echo "{\"label\":\"$label\",\"load_s\":$load,\"vram_after_load_mib\":$idle}" | tee $OUT/$label.load.json
eval /w/client/bin/python /w/bench.py --url http://127.0.0.1:$port --label $label --tokenizer $tokdir --out $OUT $bargs
kill -- -$srv 2> /dev/null
for _ in $(seq 1 60); do kill -0 $srv 2> /dev/null || break; sleep 1; done
kill -9 -- -$srv 2> /dev/null
kill $smi
peak=$(cut -d, -f1 $OUT/$label.gpu.csv | sort -n | tail -1)
echo "{\"label\":\"$label\",\"peak_vram_mib\":$peak}" | tee -a $OUT/$label.load.json
sleep 3
