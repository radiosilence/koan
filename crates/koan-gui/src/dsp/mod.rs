//! EQ and convolution: profiles per output device, the response graph and
//! its editor, the band table, and every way a profile comes in. The page a
//! window shows is `EqPage`; everything else opens over it as a sheet.

pub mod bands;
pub mod flows;
pub mod graph;
pub mod manage;
pub mod menu;
pub mod model;
pub mod page;
pub mod profile;
pub mod prompts;
pub mod sheet;
pub mod stage;

pub use model::Dsp;
pub use page::EqPage;
