//! Cyclone II EP2C35 bitstream addressing and assembly. Device facts come
//! from the `ddb` crate; this crate owns where they land in an RBF.
pub mod addr;
pub mod blank;
pub mod chipdb;
pub mod lab;
pub mod logic;
pub mod vo;
pub mod le;
pub mod io;
pub mod rbf;
pub mod nextpnr;
pub mod phys;
pub mod quartus;
pub mod rcf;
pub mod route;
