#!/bin/bash
# setup.sh: on the pod, as root. Data, the client venv, uv. Engines and models are installed by their own steps.
set -eu
mkdir -p /w/data/images /w/data/videos /w/out /w/models
cd /w/data/images
n=0
for id in $(seq 10 70); do
  [ $n -ge 32 ] && break
  f=$(printf 'p%03d.jpg' $id)
  [ -s $f ] || curl -sfL --retry 3 -o $f "https://picsum.photos/id/$id/1280/720" || { rm -f $f; continue; }
  file $f | grep -q JPEG || { rm -f $f; continue; }
  n=$((n + 1))
done
ls | head -40 | wc -l
sha256sum *.jpg > /w/data/images.sha256
cd /w/data/videos
for v in bigbuckbunny/mp4/h264/720/Big_Buck_Bunny_720_10s_1MB.mp4 jellyfish/mp4/h264/720/Jellyfish_720_10s_1MB.mp4 sintel/mp4/h264/720/Sintel_720_10s_1MB.mp4; do
  [ -s ${v##*/} ] || curl -sfL --retry 3 -o ${v##*/} "https://test-videos.co.uk/vids/$v"
done
sha256sum *.mp4 > /w/data/videos.sha256
command -v uv > /dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
export PATH=$HOME/.local/bin:$PATH
[ -x /w/client/bin/python ] || { uv venv -q -p 3.12 /w/client && uv pip install -q -p /w/client/bin/python aiohttp tokenizers pillow "huggingface_hub[hf_xet]"; }
echo setup ok
