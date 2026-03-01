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

## Function reference

### String

| Function | Args | Result |
|---|---|---|
| `$lower(s)` | string | lowercase |
| `$upper(s)` | string | UPPERCASE |
| `$caps(s)` | string | Title Case |
| `$trim(s)` | string | strip whitespace |
| `$left(s,n)` | string, count | first n chars |
| `$right(s,n)` | string, count | last n chars |
| `$pad(s,n)` | string, width | right-align (pad left with spaces) |
| `$pad_right(s,n)` | string, width | left-align (pad right with spaces) |
| `$replace(s,from,to)` | string, search, replacement | string replace |
| `$len(s)` | string | character count |

### Logic

| Function | Args | Result |
|---|---|---|
| `$if(cond,then,else)` | test, if non-empty, if empty | conditional |
| `$if2(a,b)` | primary, fallback | first non-empty |
| `$if3(a,b,c,...)` | values... | first non-empty of any |

### Numeric

| Function | Args | Result |
|---|---|---|
| `$num(n,digits)` | number, width | zero-padded (`$num(5,3)` → `005`) |
| `$div(a,b)` | dividend, divisor | integer division |
| `$mod(a,b)` | dividend, divisor | remainder |

### Path

| Function | Args | Result |
|---|---|---|
| `$directory(path)` | file path | parent directory name |
| `$directory_path(path)` | file path | full parent path |
| `$ext(path)` | file path | file extension |
| `$filename(path)` | file path | filename without extension |

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
koan organize --pattern '[$genre%/]%album artist%/%album%/%tracknumber%. %title%'
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

**Functions:** `$abbr`, `$add`, `$sub`, `$mul`, `$max`, `$min`, `$greater`, `$muldiv`, `$ifequal`, `$ifgreater`, `$iflonger`, `$select`, `$caps2`, `$insert`, `$substr`, `$repeat`, `$stripprefix`, `$swapprefix`, `$strcmp`, `$stricmp`, `$longer`, `$longest`, `$shortest`, `$strchr`, `$strrchr`, `$strstr`, `$hex`, `$roman`, `$year`, `$month`, `$day_of_month`, `$date`, `$time`, `$char`, `$crc32`, `$ansi`, `$ascii`, `$fix_eol`, `$padcut`, `$padcut_right`, `$meta`, `$meta_sep`, `$meta_num`, `$meta_test`, `$info`, `$channels`, `$get`, `$put`, `$puts`, `$and`, `$or`, `$not`, `$xor`, `$tab`, `$crlf`, `$progress`, `$progress2`, `$rand`, `$rot13`, `$blend`, `$transition`, `$rgb`, `$hsl`

**Fields:** `%track number%` (unpadded), `%totaltracks%`, `%totaldiscs%`, `%samplerate%`, `%bitrate%`, `%channels%`, `%filename%`, `%filename_ext%`, `%directoryname%`, `%path%`, `%filesize%`, `%filesize_natural%`, `%length%`, `%length_seconds%`, all ReplayGain fields, all playback status fields
