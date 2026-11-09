//! `svdq.nvfp4`'s producer: SVDQuant's fit (`quantize`).

#[cfg(feature = "quantize")]
#[path = "svdq_fit.rs"]
mod fit;
#[cfg(feature = "quantize")]
pub(crate) use fit::{
    SVDQ_FIT_STRATEGY, svdq_fit, svdq_fit_workspace,
};
