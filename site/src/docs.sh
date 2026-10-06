#!/usr/bin/env bash
# Renders the markdown in docs/, and the changelog, into public/docs/<name>/.
# The markdown stays where it is so GitHub can render it too; this is the copy
# the site serves. Run from site/ (`mise run docs`).
set -euo pipefail

root=..
out=public/docs

# Section | source. A file in docs/ that is not listed here is not published.
pages=(
  "Start|docs/getting-started.md"
  "Start|docs/guide/migrating-from-navidrome.md"
  "Server|docs/guide/headless-server.md"
  "Server|docs/guide/authentication.md"
  "Server|docs/guide/remote-servers.md"
  "Library|docs/guide/file-organization.md"
  "Library|docs/guide/smart-playlists.md"
  "Library|docs/format-strings.md"
  "Library|docs/recipes/cache-management.md"
  "Playback|docs/guide/devices.md"
  "Playback|docs/guide/apple-tv.md"
  "Playback|docs/guide/sleep-timer.md"
  "Playback|docs/guide/dsp.md"
  "Automation|docs/guide/mcp-integration.md"
  "Automation|docs/guide/graphql-api.md"
  "Reference|docs/reference/configuration.md"
  "Reference|docs/reference/cli.md"
  "Reference|docs/reference/keybindings.md"
  "Reference|docs/recipes/troubleshooting.md"
  "Reference|CHANGELOG.md"
)

slug() { basename "$1" .md | tr '[:upper:]' '[:lower:]'; }
title() { sed -n 's/^# //p' "$root/$1" | head -1; }

# The sidebar, with the page being rendered marked current.
nav() {
  local current=$1 section="" html=""
  for entry in "${pages[@]}"; do
    local s=${entry%%|*} src=${entry#*|}
    if [[ $s != "$section" ]]; then
      [[ -n $section ]] && html+="</ul>"
      html+="<h2>$s</h2><ul>"
      section=$s
    fi
    local name aria=""
    name=$(slug "$src")
    [[ $name == "$current" ]] && aria=' aria-current="page"'
    html+="<li><a href=\"/docs/$name/\"$aria>$(title "$src")</a></li>"
  done
  printf '%s</ul>' "$html"
}

render() {
  local name=$1
  shift
  mkdir -p "$out/$name"
  pandoc --from gfm --to html5 --standalone \
    --template src/docs.html --lua-filter src/docs.lua \
    --syntax-highlighting=pygments --columns=10000 --variable "nav=$(nav "$name")" \
    --variable "path=/docs/${name:+$name/}" \
    --output "$out/$name/index.html" "$@"
}

rm -rf "$out"
for entry in "${pages[@]}"; do
  src=${entry#*|}
  render "$(slug "$src")" --toc --toc-depth=2 < "$root/$src"
done

# The index: the same list as the sidebar, as the page.
{
  echo "# Documentation"
  section=""
  for entry in "${pages[@]}"; do
    s=${entry%%|*} src=${entry#*|}
    if [[ $s != "$section" ]]; then
      printf '\n## %s\n\n' "$s"
      section=$s
    fi
    echo "- [$(title "$src")](/docs/$(slug "$src")/)"
  done
} | render ""

# A link to a page that was not rendered, or to a heading the page does not
# have, is a typo or a page missing from the list above.
dead=$(grep -rhoE 'href="/docs/[^"]*"' "$out" public/*.html | sed 's/^href="//; s/"$//' | sort -u | while read -r link; do
  path=${link%%#*}
  page=public${path%/}/index.html
  if [[ ! -f $page ]]; then
    echo "$link"
  elif [[ $link == *#* ]] && ! grep -q "id=\"${link#*#}\"" "$page"; then
    echo "$link"
  fi
done)
if [[ -n $dead ]]; then
  echo "Links to pages or headings that do not exist:" >&2
  echo "$dead" >&2
  exit 1
fi
