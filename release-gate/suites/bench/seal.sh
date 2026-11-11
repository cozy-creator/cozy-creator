#!/bin/bash
# seal.sh <gate dir>: the ComfyUI column, sealed for the owner (Paul 11-10: ComfyUI's numbers stay off public places until
# every cell is won; a public repo's CI logs and artifacts are public). Every file that holds ComfyUI's numbers goes into
# comfyui.sealed (AES-256 under a fresh key) and the key into comfyui.sealed.key (RSA-OAEP to comfyui-seal.pub); the
# plaintext is removed. Open it where the private key is:
#   openssl pkeyutl -decrypt -pkeyopt rsa_padding_mode:oaep -inkey KEY -in comfyui.sealed.key |
#     openssl enc -d -aes-256-cbc -pbkdf2 -pass stdin -in comfyui.sealed | tar -xzf -
set -euo pipefail
cd "$1"
files=()
for f in comfyui comfyui-rows.json comfyui.md bench-table.md; do [ ! -e "$f" ] || files+=("$f"); done
[ ${#files[@]} -gt 0 ] || exit 0
key=$(openssl rand -hex 32)
tar -czf - "${files[@]}" | openssl enc -aes-256-cbc -pbkdf2 -pass "pass:$key" -out comfyui.sealed
printf %s "$key" | openssl pkeyutl -encrypt -pubin -inkey "$(dirname "$0")/comfyui-seal.pub" -pkeyopt rsa_padding_mode:oaep -out comfyui.sealed.key
rm -rf "${files[@]}"
echo "sealed: ${files[*]}"
