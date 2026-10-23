//! Quartus II 13.0 `.ddb` device database reader (Cyclone II). Device facts
//! only: the routing graph, wire names and the flat CRAM addresses each
//! routing connection sets. Where those addresses land in a bitstream is the
//! `cram` crate's business.
pub mod asm;
pub mod asmdb;
pub mod container;
pub mod device;
pub mod dtm;
pub mod export;
pub mod icname;
pub mod pdb;
pub mod pins;
pub mod routing;
pub mod rtiming;
