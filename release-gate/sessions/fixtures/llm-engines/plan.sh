#!/bin/bash
# plan.sh <config…>: each engine config through run.sh, in order. Results in /w/out/<config>.*
cd /w
NOCACHE='{"cache_prompt": false}'
V="env CPATH=/w/vllm/lib/python3.12/site-packages/nvidia/cu13/include LIBRARY_PATH=/w/vllm/lib/python3.12/site-packages/nvidia/cu13/lib /w/vllm/bin/vllm"
VCOMMON="--served-model-name m --max-model-len 16384 --gpu-memory-utilization 0.90 --no-enable-prefix-caching --mm-processor-cache-gb 0 --max-num-seqs 32 --limit-mm-per-prompt {\"image\":1,\"video\":1} --media-io-kwargs {\"video\":{\"num_frames\":20}}"
SG="env CPATH=/w/sglang/lib/python3.12/site-packages/nvidia/cu13/include LIBRARY_PATH=/w/sglang/lib/python3.12/site-packages/nvidia/cu13/lib /w/sglang/bin/python -m sglang.launch_server"
SCOMMON="--served-model-name m --mem-fraction-static 0.85 --disable-radix-cache --context-length 16384 --max-running-requests 32"
L=$(find /w/llamabin -name llama-server | head -1)
export LD_LIBRARY_PATH=$(dirname "$L"):$(find /w/llamabin -maxdepth 1 -type d -name "cudart-*" | head -1):${LD_LIBRARY_PATH:-}
LCOMMON="--alias m -ngl 999 -c 65536 -np 32 --jinja -fa on --cache-ram 0 --video-fps 2"
MTPV='--speculative-config {"method":"mtp","num_speculative_tokens":2}'
MTPS="--speculative-algorithm EAGLE --speculative-num-steps 2 --speculative-eagle-topk 1 --speculative-num-draft-tokens 3"
G9=/w/models/9b-gguf G27=/w/models/27b-q4
R9=qwen/qwen3.5-9b@sha256:37a131b745e22e0c074592f349d6fd6778e5b127dd2a4b277e64ffd053742bd7
R27=heretic-org/qwen3.8-27b-heretic-ara@sha256:f33791610f19a35d22bc7c5969e49c5064dce025d58ef6726d4de2e24931a8e5
VTIGHT="--served-model-name m --max-model-len 2048 --kv-cache-memory-bytes 1500000000 --no-enable-prefix-caching --mm-processor-cache-gb 0 --max-num-seqs 8 --limit-mm-per-prompt {\"image\":1,\"video\":0} --mm-processor-kwargs {\"max_pixels\":921600} --enforce-eager"
for c in "$@"; do
  case $c in
    vllm-9b-bf16) bash run.sh $c 8000 /w/models/9b "" -- $V serve /w/models/9b --port 8000 $VCOMMON ;;
    vllm-9b-bf16-mtp) bash run.sh $c 8000 /w/models/9b "" -- $V serve /w/models/9b --port 8000 $VCOMMON $MTPV ;;
    vllm-9b-fp8) bash run.sh $c 8000 /w/models/9b "" -- $V serve /w/models/9b-fp8 --port 8000 $VCOMMON ;;
    vllm-27b-nvfp4) bash run.sh $c 8000 /w/models/9b "" -- $V serve /w/models/27b-nvfp4 --port 8000 $VCOMMON ;;
    vllm-27b-nvfp4-mtp) bash run.sh $c 8000 /w/models/9b "" -- $V serve /w/models/27b-nvfp4 --port 8000 $VCOMMON $MTPV ;;
    sglang-9b-bf16) bash run.sh $c 8001 /w/models/9b "" -- $SG --model-path /w/models/9b --port 8001 $SCOMMON ;;
    sglang-9b-bf16-mtp) bash run.sh $c 8001 /w/models/9b "" -- $SG --model-path /w/models/9b --port 8001 $SCOMMON $MTPS ;;
    sglang-27b-nvfp4) bash run.sh $c 8001 /w/models/9b "" -- $SG --model-path /w/models/27b-nvfp4 --port 8001 $SCOMMON --quantization modelopt_fp4 ;;
    llama-9b-q8) bash run.sh $c 8002 /w/models/9b "--extra '$NOCACHE'" -- $L -m $G9/Qwen3.5-9B-Q8_0.gguf --mmproj $G9/mmproj-BF16.gguf --port 8002 $LCOMMON ;;
    llama-9b-q8-mtp) bash run.sh $c 8002 /w/models/9b "--extra '$NOCACHE'" -- $L -m $G9/Qwen3.5-9B-Q8_0.gguf --mmproj $G9/mmproj-BF16.gguf --port 8002 $LCOMMON --spec-type draft-mtp --spec-draft-n-max 2 ;;
    llama-27b-q4) bash run.sh $c 8002 /w/models/9b "--extra '$NOCACHE'" -- $L -m $G27/Qwen3.8-27B-Q4_K_M.gguf --mmproj $G27/mmproj-Qwen3.8-27B-Q8_0.gguf --port 8002 $LCOMMON ;;
    llama-27b-q4-mtp) bash run.sh $c 8002 /w/models/9b "--extra '$NOCACHE'" -- $L -m $G27/Qwen3.8-27B-Q4_K_M.gguf --mmproj $G27/mmproj-Qwen3.8-27B-Q8_0.gguf --model-draft $G27/mtp-Qwen3.8-27B-Q8_0.gguf --port 8002 $LCOMMON --spec-type draft-mtp --spec-draft-n-max 2 ;;
    vllm-9b-convrot) bash run.sh $c 8000 /w/models/9b "" -- env COZY_TENSORFS_DIR=/var/lib/tensorfs VLLM_DISABLED_KERNELS=CutlassInt8ScaledMMLinearKernel $V serve /w/models/9b-cozy --port 8000 $VCOMMON --load-format cozytensors --model-loader-extra-config {\"ref\":\"$R9\"} ;;
    vllm-27b-convrot) bash run.sh $c 8000 /w/models/9b "" -- env COZY_TENSORFS_DIR=/var/lib/tensorfs VLLM_DISABLED_KERNELS=CutlassInt8ScaledMMLinearKernel $V serve /w/models/27b-cozy --port 8000 $VTIGHT --load-format cozytensors --model-loader-extra-config {\"ref\":\"$R27\"} ;;
    vllm-27b-fp8) bash run.sh $c 8000 /w/models/9b "" -- $V serve /w/models/27b-fp8 --port 8000 $VTIGHT ;;
    *) echo "unknown $c" ;;
  esac
done
