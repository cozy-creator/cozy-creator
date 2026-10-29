# xs-133 package releases on Runtime 0.35.2 (packages #715 + #716), proof-only. Sourced by gate.sh --cells after the
# packages suite's product cells: the packages that have none, each once on its default lane, then hunyuan-image-3's
# first real run since #716 (88 GB int8 streamed through a 24 GB card).
cell krea-2 "$(pkg krea-2 generate_turbo)" prompt="$OBJECT, studio photo" seed:=$((R + 30))
cell ideogram-4 "$(pkg ideogram-4 generate)" prompt="A poster that reads 'COZY 0715' above $OBJECT" seed:=$((R + 31))
cell flux-2 "$(pkg flux-2 generate_turbo)" prompt="$SCENE, photo" seed:=$((R + 32))
cell qwen3.6 "$(pkg qwen3.6 ask)" prompt="In one sentence: what is a release gate for?"
cell hunyuan "$(pkg hunyuan-image-3 generate_turbo)" prompt="$CHAR, cinematic portrait" seed:=$((R + 33))
