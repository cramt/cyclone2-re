//! Cyclone II EP2C35 bitstream reverse engineering. The assembler itself
//! (routed design -> RBF -> JTAG stream) is chipsmith's
//! (`chipsmith_cyclone2::bitstream`); this crate derives the device database
//! it reads from Quartus' files (`db`, `chipdb`), reads Quartus' own
//! designs into it (`quartus`, `rcf`, `vo`, `route`) and checks the result
//! against Quartus.
pub mod chipdb;
pub mod db;
pub mod quartus;
pub mod rcf;
pub mod route;
pub mod vo;
