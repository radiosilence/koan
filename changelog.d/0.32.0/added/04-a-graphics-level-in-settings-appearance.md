- **A graphics level, in Settings -> Appearance.** One slider from `Plain` to `Full`, so koan can be told how much of the machine it may spend on looking like itself. `Full` is what it has always done and stays the default.

  | | Wash | Indicators | Chrome |
  |---|---|---|---|
  | `Full` | drifts | dance | glass |
  | `Reduced` | held still | dance | glass |
  | `Plain` | none | held still | flat materials |

  The steps are ordered by what they were measured to cost rather than by how much they look like they cost. On an M1 Pro, playing, window frontmost: `Full` is 15-18% of a core, `Reduced` around 9%, `Plain` around 6%.

  `Reduced` is where it is because of what the wash's cost actually is. The blur is rasterised once at 360 points and magnified as a texture, so it is close to free -- what is expensive is the *drift*: three animations of incommensurate period running forever on scale, rotation and offset, which is the only thing making the wash's composited output differ frame to frame, so the copy mirrored out under the sidebar and toolbar and the glass sampling it are redone for as long as the music runs. Held still, the wash measures the same as no wash at all. The record's colour is free; only the breathing is billed.

  Below that, `Plain` is for machines where drawing a blurred backdrop is dear at all. Most of what it saves over `Reduced` is the playing indicators standing still, which stops a 30fps timeline and the analyser poll behind it. It also swaps the glass chrome for flat materials, which measured about a point of a core on top -- close enough to the noise floor that it is there on the reasoning rather than on the evidence, for GPUs and display sizes unlike this one.

  The setting lives in macOS defaults (`defaults write cc.blit.koan graphics -int 0`) rather than `config.toml`: how much this app draws is this machine's business, and the TUI has none of it to draw.
