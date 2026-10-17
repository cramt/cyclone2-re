//! Cyclone II EP2C35 bitstream addressing and assembly. Device facts come
//! from the `ddb` crate; this crate owns where they land in an RBF.
pub mod addr;
pub mod design;
pub mod le;
mod le_mux_table;
pub mod rbf;
pub mod rcf;
pub mod route;
