# Format Strings

fb2k-compatible format string engine used by `koan organize` and library display formatting.

## Syntax

Three constructs: **fields**, **conditionals**, and **functions**. Everything else is literal text (including `/` for directory separators).

### Fields: `%field name%`

Replaced with the track's metadata value. If the field doesn't exist, it evaluates to empty string.

```
%artist% - %title%
→ Aphex Twin - Windowlicker
```

Available fields:

| Field | Example |
|---|---|
| `%title%` | Windowlicker |
| `%artist%` | Aphex Twin |
| `%album artist%` | Aphex Twin |
| `%album%` | Windowlicker EP |
| `%date%` | 1999 |
| `%tracknumber%` | 01 (zero-padded) |
| `%discnumber%` | 1 |
| `%codec%` | FLAC |
| `%genre%` | Electronic |

### Conditionals: `[...]`

Content inside brackets is only included if **all** fields within it have values. If any field is missing, the entire block is omitted.

```
%artist%[ - %album%]
→ Aphex Twin - Windowlicker EP   (album exists)
→ Aphex Twin                     (no album tag)
```

Nest them for finer control:

```
%artist%[ (%date%)][ - %album%]
→ Aphex Twin (1999) - Windowlicker EP   (date + album)
→ Aphex Twin - Windowlicker EP          (no date)
→ Aphex Twin (1999)                     (no album)
→ Aphex Twin                            (neither)
```

### Functions: `$function(args)`

Transform values. Arguments are comma-separated. Fields can be used as arguments.

```
$upper(%artist%)
→ APHEX TWIN

$if(%date%,%date%,Unknown)
→ 1999       (date exists)
→ Unknown    (no date)
```

### Quoted literals: `'...'`

Single quotes escape special characters (`[`, `]`, `%`, `$`, `,`, `(`). Use them when you need literal brackets in output:

```
%album% '['%codec%']'
→ OK Computer [FLAC]
```

## Function reference

### String

| Function | Args | Result |
|---|---|---|
| `$lower(s)` | string | lowercase |
| `$upper(s)` | string | UPPERCASE |
| `$caps(s)` | string | Title Case |
| `$caps2(s)` | string | Title Case (articles stay lowercase) |
| `$trim(s)` | string | strip whitespace |
| `$left(s,n)` | string, count | first n chars |
| `$right(s,n)` | string, count | last n chars |
| `$substr(s,from,to)` | string, start, end | substring (0-indexed) |
| `$pad(s,n)` | string, width | right-align (pad left with spaces) |
| `$pad_right(s,n)` | string, width | left-align (pad right with spaces) |
| `$padcut(s,n)` | string, width | right-align, truncate to width |
| `$padcut_right(s,n)` | string, width | left-align, truncate to width |
| `$insert(s,sub,pos)` | string, substring, position | insert at position |
| `$replace(s,from,to)` | string, search, replacement | string replace |
| `$repeat(s,n)` | string, count | repeat n times |
| `$len(s)` | string | character count |
| `$abbr(s)` | string | first letter of each word |
| `$stripprefix(s)` | string | remove leading "A "/"The " |
| `$swapprefix(s)` | string | "The Beatles" → "Beatles, The" |
| `$rot13(s)` | string | ROT13 cipher |
| `$fix_eol(s)` | string | replace line breaks with spaces |
| `$fix_eol(s,r)` | string, replacement | replace line breaks with r |

### String search

| Function | Args | Result |
|---|---|---|
| `$strchr(s,c)` | string, char | position of first occurrence (1-indexed, "" if not found) |
| `$strrchr(s,c)` | string, char | position of last occurrence |
| `$strstr(s,sub)` | string, substring | position of substring |

### Comparison (boolean)

These return `"1"` for true, `""` for false — designed for use with `$if()`.

| Function | Args | Result |
|---|---|---|
| `$strcmp(a,b)` | string, string | true if equal (case-sensitive) |
| `$stricmp(a,b)` | string, string | true if equal (case-insensitive) |
| `$longer(a,b)` | string, string | true if a is longer |
| `$longest(a,b,...)` | strings | returns the longest |
| `$shortest(a,b,...)` | strings | returns the shortest |
| `$greater(a,b)` | number, number | true if a > b |
| `$not(x)` | value | invert boolean |
| `$and(a,b)` | value, value | both non-empty |
| `$or(a,b)` | value, value | either non-empty |
| `$xor(a,b)` | value, value | exactly one non-empty |

### Logic

| Function | Args | Result |
|---|---|---|
| `$if(cond,then,else)` | test, if non-empty, if empty | conditional |
| `$if2(a,b)` | primary, fallback | first non-empty |
| `$if3(a,b,c,...)` | values... | first non-empty of any |
| `$ifequal(a,b,then,else)` | int, int, if equal, if not | numeric equality |
| `$ifgreater(a,b,then,else)` | int, int, if a>b, if not | numeric comparison |
| `$iflonger(s,n,then,else)` | string, length, if longer, if not | length comparison |
| `$select(n,a,b,c,...)` | index (1-based), values... | select nth value |

### Numeric

| Function | Args | Result |
|---|---|---|
| `$num(n,digits)` | number, width | zero-padded (`$num(5,3)` → `005`) |
| `$add(a,b)` | int, int | addition |
| `$sub(a,b)` | int, int | subtraction |
| `$mul(a,b)` | int, int | multiplication |
| `$div(a,b)` | int, int | integer division |
| `$mod(a,b)` | int, int | remainder |
| `$muldiv(a,b,c)` | int, int, int | (a*b)/c without overflow |
| `$max(a,b)` | int, int | larger value |
| `$min(a,b)` | int, int | smaller value |
| `$hex(n)` | int | hexadecimal |
| `$hex(n,digits)` | int, width | zero-padded hex |

### Path

| Function | Args | Result |
|---|---|---|
| `$directory(path)` | file path | parent directory name |
| `$directory_path(path)` | file path | full parent path |
| `$ext(path)` | file path | file extension |
| `$filename(path)` | file path | filename without extension |

### Special

| Function | Args | Result |
|---|---|---|
| `$tab()` | (optional count) | tab character(s) |
| `$crlf()` | — | newline |
| `$char(n)` | code point | unicode character |

## Named patterns

Store patterns in config and reference them by name instead of typing the full format string every time.

`~/.config/koan/config.toml`:

```toml
[organize]
default = "standard"

[organize.patterns]
standard = "%album artist%/(%date%) %album%/%tracknumber%. %title%"
va-aware = "%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%"
label = "$if2(%label%,%album artist%)/%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%"
```

```bash
koan organize                    # uses default pattern
koan organize --pattern va-aware # use named pattern
koan organize --list             # show all configured patterns
```

If `--pattern` doesn't match a named pattern, it's treated as a raw format string.

## Organize examples

Slashes in the pattern create directory structure. `--execute` applies, default is preview.

### Standard layout

```bash
koan organize --pattern '%album artist%/(%date%) %album%/%tracknumber%. %title%'
```

```
Aphex Twin/(1999) Windowlicker EP/01. Windowlicker.flac
Aphex Twin/(1999) Windowlicker EP/02. Nannou.flac
```

### With disc number for multi-disc albums

```bash
koan organize --pattern '%album artist%/[(%date%) ]%album%/[%discnumber%-]%tracknumber%. %title%'
```

```
Tool/(2006) 10,000 Days/1-01. Vicarious.flac
Tool/(2006) 10,000 Days/2-01. Wings for Marie.flac
```

The `[%discnumber%-]` conditional means the disc prefix only appears if `discnumber` exists.

### VA-aware layout with $stricmp

Skip the date prefix for Various Artists compilations:

```bash
koan organize --pattern '%album artist%/$if($stricmp(%album artist%,Various Artists),,'\''('\''$left(%date%,4)'\'')'\'')%album% '\''['\''\''%codec%'\'''\'']'\''/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%'
```

Or in a config file where quoting is simpler:

```
%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%
```

```
Aphex Twin/(1992) Selected Ambient Works 85-92 [FLAC]/01. Aphex Twin - Xtal.flac
Various Artists/Warp 20 Recreated [FLAC]/03. Flying Lotus - Roygbiv.flac
```

### Label-based with $if2 fallback

```bash
koan organize --pattern '$if2(%label%,%album artist%)/%album% '\''['\''\''%codec%'\'''\'']'\''/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%'
```

```
Warp Records/Selected Ambient Works 85-92 [FLAC]/01. Aphex Twin - Xtal.flac
```

Uses record label if tagged, falls back to album artist.

### Artist - Title flat layout

```bash
koan organize --pattern '%artist% - %title%'
```

```
Aphex Twin - Windowlicker.flac
Boards of Canada - Roygbiv.flac
```

### Fallback artist with $if2

```bash
koan organize --pattern '$if2(%album artist%,%artist%)/%album%/%tracknumber%. %title%'
```

Uses album artist if available, falls back to track artist.

### Genre-based structure

```bash
koan organize --pattern '[%genre%/]%album artist%/%album%/%tracknumber%. %title%'
```

```
Electronic/Aphex Twin/Windowlicker EP/01. Windowlicker.flac
```

Tracks without a genre tag skip the genre directory.

## Safety

- Default is always **preview** (dry-run) — shows what would be moved without touching anything
- `--execute` previews first, then asks for **confirmation** before applying
- `--undo` reverts the last batch of moves (tracked in the database)
- `--yes` / `-y` skips the confirmation prompt (for scripts)
- Illegal filename characters (`/ \ : * ? " < > |`) are replaced with `_`
- Filenames are capped at 240 bytes (macOS limit)
- Ancillary files (cover art, .cue, .log) are moved with the music automatically
- Empty source directories are cleaned up after moves

## fb2k compatibility

The format engine implements a subset of the [foobar2000 title formatting](https://wiki.hydrogenaudio.org/index.php?title=Foobar2000:Title_Formatting_Reference) spec. The core syntax (`%fields%`, `[conditionals]`, `$functions()`) is fully compatible. Not all functions are implemented yet — PRs welcome.

### Not yet implemented

**Functions:** `$meta`, `$meta_sep`, `$meta_num`, `$meta_test`, `$info` (multi-value), `$channels`, `$get`, `$put`, `$puts`, `$progress`, `$progress2`, `$rand`, `$blend`, `$transition`, `$rgb`, `$hsl`, `$year`, `$month`, `$day_of_month`, `$date`, `$time`, `$crc32`, `$ansi`, `$ascii`

**Fields:** `%track number%` (unpadded), `%totaltracks%`, `%totaldiscs%`, `%samplerate%`, `%bitrate%`, `%channels%`, `%filename%`, `%filename_ext%`, `%directoryname%`, `%path%`, `%filesize%`, `%filesize_natural%`, `%length%`, `%length_seconds%`, all ReplayGain fields, all playback status fields
