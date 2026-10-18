//! Cyclone II EP2C35 bitstream addressing and assembly. Device facts come
//! from the `ddb` crate; this crate owns where they land in an RBF.
pub mod addr;
pub mod design;
pub mod lab;
pub mod logic;
pub mod vo;
pub mod le;
pub mod io;
pub mod rbf;
pub mod rcf;
pub mod route;
