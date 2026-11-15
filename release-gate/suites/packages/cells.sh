# Suite `packages`: only the packages a release changes, from the --packages checkout's source: each whose version there
# is a prerelease (the RC's candidates, 0.13.0rc2) and those --only names; each package's cells from products.sh, plus the
# SDXL smoke. A cell another suite already ran is not run again.
changed=$([ -z "$PKGS" ] || for f in "$PKGS"/*/pyproject.toml; do
  grep -qs cozy.application "$f" && grep -m1 '^version = ' "$f" | grep -qE '(a|b|rc)[0-9]+"|\.dev[0-9]+"' && basename "$(dirname "$f")"
done)
want=" sdxl $(echo $changed) $(tr , ' ' <<< "$ONLY") "
for p in "${PRODUCTS[@]}"; do [[ $want != *" $p "* ]] || product "$p"; done
for p in $want; do [[ " ${PRODUCTS[*]} " == *" $p "* ]] || log "packages: $p has no product cells"; done
