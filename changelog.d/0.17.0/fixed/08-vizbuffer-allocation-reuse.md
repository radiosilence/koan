- **VizBuffer allocation reuse** — added `VizBuffer::snapshot_into(&self, out: &mut Vec<f32>)` to reuse caller's buffer instead of allocating per frame ([#76](https://github.com/radiosilence/koan/pull/76))

