#!/usr/bin/env bash
# patch-installers-fix.sh <dir>: the installers in <dir> run `cozy doctor --fix` once cozy is in place (cl-1030), which installs the
# system packages cozy needs (asking once on the terminal before sudo, else saying the command) and fetches what cozy otherwise
# fetches at first need. Its stdin is never the piped script, and nothing it says or exits with fails the install.
# patch-installers.sh runs it; e2e/cli/fresh.sh runs it over a released installer on Linux, macOS and Windows (perl, not sed -i),
# which since 0.26.0 has it already: an installer that has it is left as it is. Fails when dist's text moved.
set -euo pipefail
sh=$1/cozy-installer.sh ps=$1/cozy-installer.ps1
export FIX='    "$_install_dir/cozy" doctor --fix < /dev/null || true'
grep -q -F "$FIX" "$sh" || perl -pi -e 's/^(    say "cozy is installed in .*")$/$1\n$ENV{FIX}/' "$sh"
test "$(grep -c -F "$FIX" "$sh")" = 1
export PSFIX='  try { & (Join-Path $dest_dir "cozy.exe") doctor --fix } catch { Write-Information "cozy doctor --fix: $_" }
  $global:LASTEXITCODE = 0'
grep -q -F '(Join-Path $dest_dir "cozy.exe") doctor --fix' "$ps" || perl -pi -e 's/^(  Write-Information "cozy is installed in .*")\r?$/$1\n$ENV{PSFIX}/' "$ps"
test "$(grep -c -F '(Join-Path $dest_dir "cozy.exe") doctor --fix' "$ps")" = 1
