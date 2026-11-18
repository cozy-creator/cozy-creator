#!/bin/bash
# seal.sh <gate dir>: the ComfyUI column, sealed for the owner (Paul 11-10: ComfyUI's numbers stay off public places until
# every cell is won; a public repo's CI logs and artifacts are public). Every file that holds ComfyUI's numbers goes into
# comfyui.sealed (AES-256 under a fresh key) and the key into comfyui.sealed.key (RSA-OAEP to comfyui-seal.pub); the
# plaintext is removed. Open it where the private key is:
#   openssl pkeyutl -decrypt -pkeyopt rsa_padding_mode:oaep -inkey KEY -in comfyui.sealed.key |
#     openssl enc -d -aes-256-cbc -pbkdf2 -pass stdin -in comfyui.sealed | tar -xzf -
# It fails closed: whatever goes wrong, the plaintext is removed (and a partial seal with it), never left to be published.
set -euo pipefail
# The key's path before the `cd`: a relative $0 (the workflow's `bash creator/...`) named nothing after it, and the plaintext
# was published (xs-176).
pub=$(cd "$(dirname "$0")" && pwd)/comfyui-seal.pub
cd "$1"
files=()
for f in comfyui comfyui-rows.json comfyui.md bench-table.md comfy-window; do [ ! -e "$f" ] || files+=("$f"); done
[ ${#files[@]} -gt 0 ] || exit 0
trap 'rc=$?; [ "$rc" = 0 ] || { rm -rf "${files[@]}" comfyui.sealed comfyui.sealed.key; echo "seal failed ($rc): the ComfyUI files are removed, unsealed" >&2; }' EXIT
[ -s "$pub" ] || { echo "no public key at $pub" >&2; exit 1; }
key=$(openssl rand -hex 32)
tar -czf - "${files[@]}" | openssl enc -aes-256-cbc -pbkdf2 -pass "pass:$key" -out comfyui.sealed
printf %s "$key" | openssl pkeyutl -encrypt -pubin -inkey "$pub" -pkeyopt rsa_padding_mode:oaep -out comfyui.sealed.key
rm -rf "${files[@]}"
echo "sealed: ${files[*]}"
