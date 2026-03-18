use proc_macro::TokenStream;
use quote::quote;
use syn::{parse::Parser, parse_macro_input, spanned::Spanned, Data, DeriveInput, Fields};

#[proc_macro_derive(Metrics, attributes(new, crate_path, config, raw_end_fn, calculate))]
pub fn derive_metrics(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive_metrics_inner(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}
fn derive_metrics_inner(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let _vis = &input.vis;

    let Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "Metrics can only be derived for structs",
        ));
    };

    let Fields::Named(fields) = &data_struct.fields else {
        return Err(syn::Error::new(
            input.ident.span(),
            "Metrics can only be derived for structs with named fields",
        ));
    };

    let crate_path = input
        .attrs
        .iter()
        .find(|a| a.path().is_ident("crate_path"))
        .map(|a| a.meta.require_list().map(|meta| meta.tokens.clone()))
        .transpose()?
        .unwrap_or_else(|| quote! { profiler });

    let mut default_fields = Vec::new();
    let mut starts = Vec::new();
    let mut results = Vec::new();
    let mut start_calls = Vec::new();
    let mut end_calls = Vec::new();
    let mut after_end_calls = Vec::new();
    let mut result_tuple_fields = Vec::new();
    let mut field_configs = Vec::new();
    let mut format_match_arms = Vec::new();
    let mut result_to_f64_match_arms = Vec::new();

    for (idx, field) in fields.named.iter().enumerate() {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let custom_new = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("new"))
            .map(|attr| attr.meta.require_list().map(|meta| meta.tokens.clone()))
            .transpose()?;

        let raw_end_fn = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("raw_end_fn"))
            .map(|attr| {
                attr.meta
                    .require_list()
                    .and_then(|meta| Ok(meta.tokens.clone()))
            })
            .transpose()?;

        if raw_end_fn.is_some() && custom_new.is_some() {
            return Err(syn::Error::new(
                field.span(),
                "Cannot use both #[new] and #[raw_end_fn] on the same field",
            ));
        }

        let configs = field
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("config"))
            .map(|a| {
                a.meta.require_list().and_then(|meta| {
                    // parse list of `name = value` pairs (comma separated)
                    let mut name_value_pairs = Vec::new();
                    let tts = meta.tokens.clone();
                    let parser = syn::punctuated::Punctuated::<
                            syn::MetaNameValue,
                            syn::Token![,],
                        >::parse_terminated;
                    let pairs = parser.parse2(tts)?;
                    for pair in pairs {
                        let name = pair.path.get_ident().ok_or_else(|| {
                            syn::Error::new(pair.path.span(), "Expected identifier for config name")
                        })?;
                        name_value_pairs.push((name.clone(), pair.value))
                    }

                    Ok(name_value_pairs)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if let Some(tokens) = custom_new {
            default_fields.push(quote! {
                #field_name: <#field_type>::new(#tokens)
            });
        } else {
            default_fields.push(quote! {
                #field_name: <#field_type as ::core::default::Default>::default()
            });
        }

        let mut config_builder = vec![];
        for (name, value) in configs.into_iter().flatten() {
            let configs = ["show_spread", "show_baseline"];
            if !configs.contains(&name.to_string().as_str()) {
                return Err(syn::Error::new(
                    name.span(),
                    format!("Unknown config option '{}'", name),
                ));
            }
            config_builder.push(quote! {
                #name: #value,
            });
        }

        let field_name_str = field_name.to_string();
        field_configs.push(quote! {
           #crate_path::metrics::MetricReportInfo {
               #(#config_builder)*
               ..#crate_path::metrics::MetricReportInfo::new(#field_name_str)
           }
        });

        starts.push(quote! { <#field_type as #crate_path::SingleMetric>::Start });
        results.push(quote! { <#field_type as #crate_path::SingleMetric>::Result });

        start_calls.push(quote! { #crate_path::SingleMetric::start(&self.#field_name) });

        let idx_lit = syn::Index::from(idx);
        if let Some(tokens) = raw_end_fn {
            // TODO: add asserts for field type. should impl `Default` and `FloatToInt`
            end_calls.push(quote! { let #field_name = #field_type::default(); });
            after_end_calls.push(quote! {
                {
                    let callback = #tokens;
                    result.#idx_lit = callback(&result);
                }
            });
        } else {
            end_calls.push(quote! { let #field_name = #crate_path::SingleMetric::end(&self.#field_name, start.#idx_lit); });
        }

        result_tuple_fields.push(quote! { #field_name });

        format_match_arms.push(quote! {
            #idx => #crate_path::SingleMetric::format_value(&self.#field_name, value)
        });

        result_to_f64_match_arms.push(quote! {
            #idx => #crate_path::SingleMetric::result_to_f64(&self.#field_name, &result.#idx_lit)
        });
    }

    let assert_name = syn::Ident::new(
        &format!("_DERIVE_ASSERT_{}", name),
        proc_macro2::Span::call_site(),
    );

    let expanded = quote! {
        impl ::core::default::Default for #name {
            fn default() -> Self {
                Self {
                    #(#default_fields),*
                }
            }
        }

        impl #name {
           const #assert_name: () = {
                const fn assert_send_sync<T: Send + Sync + 'static>() {}
                assert_send_sync::<#name>();
            };
        }

        impl #crate_path::Metrics for #name {
            type Start = (#(#starts,)*);
            type Result = (#(#results,)*);

            fn start(&self) -> Self::Start {
                use #crate_path::SingleMetric;
                (#(#start_calls,)*)
            }

            fn end(&self, start: Self::Start) -> Self::Result {
                #(#end_calls)*
                let mut result = (#(#result_tuple_fields,)*);
                #(#after_end_calls)*
                result
            }

            fn metrics_info() -> &'static [#crate_path::metrics::MetricReportInfo] {
                & const {
                    [#(#field_configs),*]
                }
            }

            fn format_value(&self, metric_idx: usize, value: f64) -> (String, &'static str) {
                match metric_idx {
                    #(#format_match_arms,)*
                    _ => #crate_path::format_unit_helper(value),
                }
            }

            fn result_to_f64(&self, metric_idx: usize, result: &Self::Result) -> f64 {
                match metric_idx {
                    #(#result_to_f64_match_arms,)*
                    _ => panic!("Invalid metric index"),
                }
            }
        }

    };

    Ok(expanded)
}
