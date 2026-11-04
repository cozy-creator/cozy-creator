#!/usr/bin/env bash
# Every route web/app calls is one Calcifer serves, by method and path, parameter names aside. Calcifer's: every
# `.route("<path>", <methods>)` under crates/calcifer/src (api.rs, project/mod.rs, hub/mod.rs), on one line or many; axum's
# `get` serves HEAD too. web/app's: every string in web/app/src starting `/v1/`, api/client.ts's above all, query dropped. Its
# method is a `method: "X"` or "X" among the call's arguments, else the call's name (`get`, `postJson`, `api.del`), else GET,
# as fetch and <img> ask. Fails on anything it can't read, so nothing goes unchecked: a route's path or methods not written
# out, a path built from a variable, no client.
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C
fail() { echo "web-routes: $*" >&2; exit 1; }
[ -f web/app/src/api/client.ts ] || fail "no web/app/src/api/client.ts"

served=$(find crates/calcifer/src -name '*.rs' -print0 | sort -z | xargs -0 perl -0777 -ne '
  my $body = qr/(?<body>(?:[^()"\/]++|\/(?!\/)|\/\/[^\n]*|"(?:[^"\\]|\\.)*"|(?<parens>\((?:[^()"\/]++|\/(?!\/)|\/\/[^\n]*|"(?:[^"\\]|\\.)*"|(?&parens))*\)))*)/;
  my $read = 0;
  while (/\.route\(\s*"([^"]+)"\s*,\s*$body\)/g) {
    my ($path, $methods) = ($1, $+{body});
    $read++;
    1 while $methods =~ s/\((?:[^()]*)\)/<>/g;
    my @verbs = $methods =~ /\b(get|head|post|put|patch|delete|options|trace|any)<>/g;
    @verbs or die "web-routes: $ARGV: the methods of $path are not written out\n";
    print uc($_), " $path\n" for @verbs;
  }
  my $all = () = /\.route\(/g;
  $all == $read or die "web-routes: $ARGV: every .route( names its path literally ($read of $all read)\n";
  /\.nest\(/ and die "web-routes: $ARGV: a .nest( hides its routes from this check\n";
') || exit 1

called=$(find web/app/src \( -name '*.ts' -o -name '*.tsx' \) -print0 | sort -z | xargs -0 perl -0777 -ne '
  # Comments, strings and templates, each whole, so one is never read inside another.
  my $str = qr/\x27(?:[^\x27\\\n]|\\.)*\x27|"(?:[^"\\\n]|\\.)*"/;
  my $tpl; $tpl = qr/`(?:[^`\\\$]|\\.|\$(?!\{)|\$\{(?:[^{}`\x27"]++|$str|(??{$tpl})|\{(?:[^{}`\x27"]++|$str|(??{$tpl}))*\})*\})*`/;
  my $args = qr/(?<rest>(?:[^()`\x27"]++|$str|$tpl|\((?&rest)\))*)/;
  my $verb = qr/GET|HEAD|POST|PUT|PATCH|DELETE/;
  my $src = $_;
  while ($src =~ /\/\/[^\n]*|\/\*.*?\*\/|($str|$tpl)/gs) {
    next unless defined $1;
    my ($lit, $at, $after) = ($1, $-[1], $+[1]);
    my $text = substr($lit, 1, -1);
    my $line = 1 + (substr($src, 0, $at) =~ tr/\n//);
    my $where = "$ARGV:$line";
    # A hole followed by a path, or a bare prefix: the route is not written out here.
    $text =~ /^\$\{[^}]*\}\/[a-z]/ and die "web-routes: $where: spell the route out whole, not $lit\n";
    $text =~ m{^/v1/?$} and die "web-routes: $where: spell the route out whole, not $lit\n";
    next unless $text =~ m{^/v1/};
    (my $path = $text) =~ s/\$\{(?:[^{}]++|\{[^{}]*\})*\}/\x01/g;
    $path =~ s/[?#].*//s;
    # The call this string is an argument of, if any: its name, what comes before, and the rest of its arguments.
    my $before = substr($src, 0, $at);
    my ($method, $name, $rest) = ("", "", "");
    if ($before =~ /([\w.\$]+)\s*(?:<[^()]*>)?\(\s*(?:($str)\s*,\s*)?$/) {
      ($name, my $first) = ($1, $2);
      $method = $1 if defined $first && $first =~ /^.($verb).$/;
      substr($src, $after) =~ /^$args\)/ and $rest = $+{rest};
    }
    if (!$method && $rest =~ /^\s*,\s*[\x27"]($verb)[\x27"]/) { $method = $1 }
    if (!$method && $rest =~ /\bmethod\s*:\s*[\x27"]($verb)[\x27"]/) { $method = $1 }
    $method ||= "?" if $rest =~ /\bmethod\b/;
    if (!$method && $name =~ /(?:^|\.)(get|head|post|put|patch|del|delete)(?:[A-Z]\w*)?$/) { $method = uc($1) eq "DEL" ? "DELETE" : uc($1) }
    $method ||= "GET";
    $path =~ s/\x01/{}/g;
    print "$method $path $where\n";
  }
') || exit 1
[ -n "$served" ] && [ -n "$called" ] || fail "no routes read"

missing=$(perl -e '
  my ($served, $called) = @ARGV;
  my @routes;
  for (split /\n/, $served) {
    my ($verb, $path) = split / /;
    (my $re = quotemeta $path) =~ s/\\\{\\\*\w+\\\}/.+/g;
    $re =~ s/\\\{\w+\\\}/[^\/]+/g;
    push @routes, [$verb, qr/^$re$/];
  }
  sub serves {
    my ($method, $shape) = @_;
    for (@routes) {
      my ($verb, $re) = @$_;
      next unless $shape =~ $re;
      return 1 if $method eq "?" || $verb eq "ANY" || $verb eq $method || ($method eq "HEAD" && $verb eq "GET");
    }
    0;
  }
  for (split /\n/, $called) {
    my ($method, $path, $where) = split / /;
    # A segment holding a hole is a parameter. The last one may end in the query instead: `/v1/runs${query}`.
    my @segments = split m{/}, $path, -1;
    my @shape = map { /\{\}/ ? "{}" : $_ } @segments;
    my @plain = @shape;
    $plain[-1] = $1 if $segments[-1] =~ /^([^{}]+)\{\}$/;
    my ($whole, $short) = (join("/", @shape), join("/", @plain));
    print "$method $path ($where)\n" unless serves($method, $whole) || serves($method, $short);
  }
' "$served" "$called")
[ -z "$missing" ] || { printf 'web-routes: web/app calls what Calcifer does not serve:\n%s\n' "$missing" >&2; exit 1; }
echo "web-routes: web/app calls $(cut -d' ' -f1,2 <<<"$called" | sort -u | wc -l) routes, all among Calcifer's $(sort -u <<<"$served" | wc -l)"
