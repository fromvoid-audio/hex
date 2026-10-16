extern crate proc_macro;
use proc_macro::TokenStream;
use proc_macro_error2::proc_macro_error;
#[proc_macro_error]
#[proc_macro]
pub fn passthrough(input: TokenStream) -> TokenStream {
    input
}
