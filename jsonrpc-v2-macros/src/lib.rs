#![recursion_limit = "2048"]

extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;

use syn::*;

struct CustomOuterAttrs(Vec<Attribute>);

impl parse::Parse for CustomOuterAttrs {
    fn parse(input: parse::ParseStream) -> Result<Self> {
        Ok(CustomOuterAttrs(input.call(Attribute::parse_outer)?))
    }
}

#[proc_macro_attribute]
pub fn jsonrpc_v2_method(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let method = parse_macro_input!(item as ItemFn);

    let method_ident = &method.sig.ident;    

    let attrs = parse_macro_input!(attrs as AttributeArgs);

    let params = &method
        .sig
        .inputs
        .iter()
        .filter_map(|x| match *x {
            FnArg::Typed(ref y) => Some(y),
            _ => None
        })
        .filter_map(|x| match *x.pat {
            Pat::Ident(ref y) => Some(y),
            _ => None
        })
        .map(|x| &x.ident)
        .collect::<Vec<_>>();

    let mut wrapped_fn_ident = None;    
    let wrapped_fn_path: Path = parse_quote!(wrapped_fn);
    let externify_path: Path = parse_quote!(externify);

    if let Some(wrapped_fn_name_lit) = attrs
        .iter()
        .filter_map(|x| match x {
            NestedMeta::Meta(y) => Some(y),
            _ => None
        })
        .filter_map(|x| match x {
            Meta::NameValue(y) => Some(y),
            _ => None
        })
        .find(|x| &x.path == &wrapped_fn_path)
        .and_then(|x| match x.lit {
            Lit::Str(ref y) => Some(y),
            _ => None
        })
    {
        wrapped_fn_ident = Some(Ident::new(&wrapped_fn_name_lit.value(), Span::call_site()));
    }

    let externify = attrs
        .iter()
        .filter_map(|x| match x {
            NestedMeta::Meta(y) => Some(y),
            _ => None
        })
        .filter_map(|x| match x {
            Meta::NameValue(y) => Some(y),
            _ => None
        })
        .find(|x| &x.path == &externify_path)
        .map(|x| match x.lit {
            Lit::Bool(ref y) => y.value,
            _ => false
        }).unwrap_or(false);

    let mut method_as_outer = quote!();

    let wrapped_fn = {

        let ItemFn { sig: Signature { inputs, output, .. }, .. } = parse_quote! {
            fn fn__(jsonrpc_v2::Params(params): jsonrpc_v2::Params<Option<jsonrpc_v2::exp::serde_json::Value>>) 
                -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<jsonrpc_v2::exp::serde_json::Value, jsonrpc_v2::Error>> + Send>> {}
        };

        let mut wrapped_fn = method.clone();
        wrapped_fn.sig.asyncness = None;
        wrapped_fn.sig.inputs = inputs;
        wrapped_fn.sig.output = output;

        if externify {

            let mut no_mangle: CustomOuterAttrs = parse_quote!(#[no_mangle]);

            wrapped_fn.attrs.append(&mut no_mangle.0);

            wrapped_fn.vis = parse_quote!(pub);
            wrapped_fn.sig.abi = Some(parse_quote!(extern));
        }

        let mut method_as_inner = quote!();
        let mut method_call_ident = method_ident.clone();
        
        if let Some(wrapped_fn_ident) = wrapped_fn_ident {
            wrapped_fn.sig.ident = wrapped_fn_ident;
            method_as_outer = quote!(#method);
        } else {
            let jsonrpc_v2_inner_ident = Ident::new("jsonrpc_v2_inner", Span::call_site());
            method_as_inner = {
                let mut method = method.clone();
                method.sig.ident = jsonrpc_v2_inner_ident.clone();
                quote!(#method)
            };
            method_call_ident = jsonrpc_v2_inner_ident.clone();
        }

        let inner_call = quote!(#method_call_ident(#(#params),*).await?);

        let block: Block = if params.is_empty() {
            parse_quote!{{
                #method_as_inner
                Box::pin(async move {
                    if params.as_ref()
                        .map(|x| x.as_object().map(|y| !y.is_empty()).unwrap_or(false) ||
                            x.as_array().map(|y| !y.is_empty()).unwrap_or(false) )
                        .unwrap_or(false) {
                        Err(jsonrpc_v2::Error::INVALID_PARAMS)
                    } else {
                        let res = #inner_call;
                        let val = jsonrpc_v2::exp::serde_json::to_value(res)?;
                        Ok(val)
                    }
                })
            }}
        } else {
            let extract_positional = extract_positional(params.len());
            let extract_named = extract_named(params.len());

            let param_names = &params
                .iter()
                .map(|id| id.to_string() )
                .collect::<Vec<_>>();

            parse_quote!{{
                #method_as_inner
                
                Box::pin(async move {
                    match params {
                        Some(jsonrpc_v2::exp::serde_json::Value::Object(map)) => {
                            #extract_named
                            if let Ok((#(#params),*)) = extract(map, #(#param_names),*) {
                                let res = #inner_call;
                                let val = jsonrpc_v2::exp::serde_json::to_value(res)?;
                                return Ok(val);
                            }
                        },
                        Some(jsonrpc_v2::exp::serde_json::Value::Array(vals)) => {
                            #extract_positional
                            if let Ok((#(#params),*)) = extract(vals) {
                                let res = #inner_call;
                                let val = jsonrpc_v2::exp::serde_json::to_value(res)?;
                                return Ok(val);
                            }
                        },
                        _ => {}
                    }
                    Err(jsonrpc_v2::Error::INVALID_PARAMS)
                })
            }}
        };

        wrapped_fn.block = Box::new(block);

        wrapped_fn
    };

    let out = quote! {
        #method_as_outer
        #wrapped_fn
    };

    // panic!("{}", &out);

    out.into()
}

fn extract_positional(up_to: usize) -> proc_macro2::TokenStream {
    let tys =
        (0..up_to).map(|i| Ident::new(&format!("T{}", i), Span::call_site())).collect::<Vec<_>>();
    let gen = tys
        .iter()
        .map(|x| quote!(#x: jsonrpc_v2::exp::serde::de::DeserializeOwned))
        .collect::<Vec<_>>();

    let ts =
        (0..up_to).map(|i| Ident::new(&format!("t{}", i), Span::call_site())).collect::<Vec<_>>();

    let mut ts_rev = ts.clone();
    ts_rev.reverse();

    let exprs = (0..up_to)
        .map(|_| quote!(jsonrpc_v2::exp::serde_json::from_value(vals.pop().unwrap()).map_err(|_| ())?))
        .collect::<Vec<_>>();

    quote! {
        fn extract<#(#gen),*>(mut vals: Vec<jsonrpc_v2::exp::serde_json::Value>) -> Result<(#(#tys),*), ()> {
            if vals.len() != #up_to {
                return Err(());
            }
            let (#(#ts_rev),*) = (#(#exprs),*);
            Ok((#(#ts),*))
        }
    }
}

fn extract_named(up_to: usize) -> proc_macro2::TokenStream {
    let tys =
        (0..up_to).map(|i| Ident::new(&format!("T{}", i), Span::call_site())).collect::<Vec<_>>();
    let gen = tys
        .iter()
        .map(|x| quote!(#x: jsonrpc_v2::exp::serde::de::DeserializeOwned))
        .collect::<Vec<_>>();

    let ts =
        (0..up_to).map(|i| Ident::new(&format!("t{}", i), Span::call_site())).collect::<Vec<_>>();

    let names =
        (0..up_to).map(|i| Ident::new(&format!("n{}", i), Span::call_site())).collect::<Vec<_>>();

    let names_and_tys = names.iter().map(|x| quote!(#x: &'static str)).collect::<Vec<_>>();

    let mains = ts
        .iter()
        .zip(names.iter())
        .map(|(t, n)| {
            quote! {
                let #t = if let Some(val) = map.remove(#n) {
                    jsonrpc_v2::exp::serde_json::from_value(val).map_err(|_| ())?
                } else {
                    return Err(());
                };
            }
        })
        .collect::<Vec<_>>();

    quote! {
        fn extract<#(#gen),*>(mut map: jsonrpc_v2::exp::serde_json::Map<String, jsonrpc_v2::exp::serde_json::Value>, #(#names_and_tys),*)
            -> Result<(#(#tys),*), ()> {
            #(#mains)*
            Ok((#(#ts),*))
        }
    }
}