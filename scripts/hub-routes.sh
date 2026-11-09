#!/usr/bin/env bash
# Every route `api::hub` calls (`Method::<VERB>, "<path>"`) is one Tensorhub's API document has, by method and path,
# parameter names aside, and every field its rental types read is in the document's schema (hub-fields, below):
# scripts/hub-routes.sh <tensorhub api/openapi.yaml>. CI reads the document at check time.
set -euo pipefail
shape() { sed -E 's/\{[^}]*\}/{}/g' | sort -u; }
documented=$(awk '/^  [^ ]/ { path = $1 } /^    (get|put|post|patch|delete):/ { print toupper($1), path }' "$1" |
  tr -d ":\"'" | { grep -E '^[A-Z]+ /' || true; } | shape)
source=$(cat "$(dirname "$0")"/../crates/api/src/hub/*.rs | tr '\n' ' ')
calls=$({ grep -oE '\.at\( *Method::[A-Z]+, *"[^"]*"' <<<"$source" || true; } | sed -E 's/.*Method::([A-Z]+), *"([^"]*)"/\1 \2/')
[ -n "$documented" ] && [ -n "$calls" ] || { echo "hub-routes: no routes read" >&2; exit 1; }
# A route not written out where it is called would go unchecked. Client::authkit_step is the one call
# that isn't `.at(`: it forwards a step AuthKit named to AuthKit's own mount, which AuthKit's contract holds.
[ "$(grep -o '\.at(' <<<"$source" | wc -l)" = "$(wc -l <<<"$calls")" ] ||
  { echo "hub-routes: every .at( names its Method and route literally" >&2; exit 1; }
called=$(shape <<<"$calls")
missing=$(comm -23 <(echo "$called") <(echo "$documented"))
if [ -n "$missing" ]; then
  printf '`api::hub` calls routes api/openapi.yaml does not have:\n%s\n' "$missing" >&2
  exit 1
fi
echo "hub-routes: $(wc -l <<<"$called") routes \`api::hub\` calls, all in api/openapi.yaml"

# `api::hub`'s rental types read only fields the document's schema of the same name has (`Type=Schema` where the names
# differ): a field the Hub never sends decodes as its default, silently. A struct-level rename or a flatten fails, unread.
held='RentalRequest RentalFallback RentalDevelopment RentalQuote RentalPriceEstimate RentalSku RentalSkuWidth
  RentalPythonInterpreter RentalListing Rental RentalFailure RentalBoot ReplannedBoot=RentalReplannedBoot
  RentalPull=RentalImagePull'
# Sent by the Hub but missing from its document (rentalcatalog.Product.Warnings); refused once the document has it.
undocumented='RentalQuote.warnings'
perl - "$1" "$held" "$undocumented" "$(dirname "$0")/../crates" <<'PERL'
use strict; use warnings;
my ($doc, $held, $undocumented, $crates) = @ARGV;
sub slurp { local (@ARGV, $/) = @_; <> }
my ($client, $protocol) = map { slurp("$crates/$_") } 'api/src/hub/types.rs', 'api/src/calcifer.rs';
my %reexported = map { $_ => 1 } (($client =~ /pub(?:\(crate\))? use crate::calcifer::\{([^}]*)\}/)[0] // '') =~ /(\w+)/g;
my ($schemas) = slurp($doc) =~ /^  schemas:\n(.*?)(?=^ {0,2}\S|\z)/ms or die "hub-fields: no components.schemas\n";
my %excused = map { $_ => 1 } split ' ', $undocumented;
my ($bad, $count) = (0, 0);
for (split ' ', $held) {
  my ($type, $schema) = split /=/; $schema //= $type;
  my $struct = qr/^((?:#\[[^\n]*\]\n)*)pub(?:\(crate\))? struct \Q$type\E \{\n(.*?)^\}/ms;
  my @found = ($reexported{$type} ? $protocol : $client) =~ /$struct/g;
  @found = $protocol =~ /$struct/g unless @found;
  @found == 2 or die "hub-fields: no one `pub struct $type`\n";
  my ($attrs, $body) = @found;
  $attrs =~ /rename_all|flatten/ and die "hub-fields: $type renames its fields\n";
  my @fields; my $field_attrs = '';
  for (split /\n/, $body) {
    next if m{^\s*//};
    if (/^\s*#\[/) { /\]\s*$/ or die "hub-fields: $type: an attribute on more than one line\n"; $field_attrs .= $_; next }
    next unless /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:r#)?(\w+)\s*:(?!:)/;
    my ($name, $a) = ($1, $field_attrs); $field_attrs = '';
    $a =~ /flatten/ and die "hub-fields: $type.$name is flattened\n";
    next if $a =~ /\bskip\b|skip_deserializing/;
    push @fields, $a =~ /\brename\s*=\s*"([^"]+)"/ ? $1 : $name;
  }
  @fields or die "hub-fields: $type: no fields read\n";
  $schemas =~ /^    \Q$schema\E:\n((?:(?:      [^\n]*)?\n)*)/m or die "hub-fields: no schema $schema\n";
  my ($block, %documented) = ($1);
  if ($block =~ /^      properties:[ \t]*\{(.*)\}[ \t]*$/m) {
    my ($depth, $token) = (0, '');
    for (split //, $1) {
      if (/[{\[]/) { $depth++ } elsif (/[}\]]/) { $depth-- }
      elsif ($depth) { next } elsif ($_ eq ',') { $token = '' }
      elsif ($_ eq ':') { $token =~ s/^\s+|\s+$//g; $documented{$token} = 1; $token = '' } else { $token .= $_ }
    }
  } elsif ($block =~ /^      properties:\n((?:(?:        [^\n]*)?\n)*)/m) {
    my $props = $1; $documented{$1} = 1 while $props =~ /^        (\w+):/mg;
  }
  %documented or die "hub-fields: $schema: no properties read\n";
  for my $field (@fields) {
    my ($known, $excuse) = ($documented{$field} ? 1 : 0, delete $excused{"$type.$field"} ? 1 : 0);
    next if $known xor $excuse;
    $bad = 1;
    print STDERR ($known ? "hub-fields: $schema documents $type.$field now: drop it from undocumented\n"
                         : "hub-fields: $type reads $field, which $schema does not have\n");
  }
  $count += @fields;
}
for (sort keys %excused) { print STDERR "hub-fields: undocumented names $_, which no type reads\n"; $bad = 1 }
exit 1 if $bad;
print "hub-fields: $count fields of `api::hub`'s rental types, all in api/openapi.yaml\n";
PERL
