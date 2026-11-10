//! Shared text contract between the renderer and the browser player.

mod blocks;
pub mod normalize;
pub mod numwords;

pub use blocks::{
    block_hash, collapse, extract_blocks, extract_marks, tokens, Block, Mark, HEADING_TAGS, SPACE_CLASS,
};
pub mod spoken_words;
