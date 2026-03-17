use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Meta};

#[proc_macro_derive(Metrics, attributes(new, register, crate_path))]
pub fn derive_metrics(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let _vis = &input.vis;

    let Data::Struct(data_struct) = &input.data else {
        panic!("Metrics can only be derived for structs");
    };

    let Fields::Named(fields) = &data_struct.fields else {
        panic!("Metrics can only be derived for structs with named fields");
    };

    let need_register = input
        .attrs
        .iter()
        .find(|a| a.path().is_ident("register"))
        .is_some();
    let crate_path = input
        .attrs
        .iter()
        .find(|a| a.path().is_ident("crate_path"))
        .map(|a| {
            {
                a.meta
                    .require_list()
                    .expect("Expected a list of tokens for crate_path")
                    .tokens
                    .clone()
            }
        })
        .unwrap_or_else(|| quote! { profiler });

    let mut default_fields = Vec::new();
    let mut starts = Vec::new();
    let mut results = Vec::new();
    let mut start_calls = Vec::new();
    let mut end_calls = Vec::new();
    let mut result_tuple_fields = Vec::new();
    let mut field_names = Vec::new();
    let mut format_match_arms = Vec::new();
    let mut result_to_f64_match_arms = Vec::new();

    for (idx, field) in fields.named.iter().enumerate() {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let mut custom_new = None;
        for attr in &field.attrs {
            if attr.path().is_ident("new") {
                if let Meta::List(list) = &attr.meta {
                    custom_new = Some(list.tokens.clone());
                }
            }
        }

        if let Some(tokens) = custom_new {
            default_fields.push(quote! {
                #field_name: <#field_type>::new(#tokens)
            });
        } else {
            default_fields.push(quote! {
                #field_name: <#field_type as ::core::default::Default>::default()
            });
        }

        starts.push(quote! { <#field_type as #crate_path::SingleMetric>::Start });
        results.push(quote! { <#field_type as #crate_path::SingleMetric>::Result });

        start_calls.push(quote! { self.#field_name.start() });

        let idx_lit = syn::Index::from(idx);
        end_calls.push(quote! { let #field_name = self.#field_name.end(start.#idx_lit); });
        result_tuple_fields.push(quote! { #field_name });

        let field_name_str = field_name.to_string();
        field_names.push(quote! { #field_name_str });

        format_match_arms.push(quote! {
            #idx => self.#field_name.format_value(value)
        });

        result_to_f64_match_arms.push(quote! {
            #idx => self.#field_name.result_to_f64(&result.#idx_lit)
        });
    }

    let assert_name = syn::Ident::new(
        &format!("_DERIVE_ASSERT_{}", name),
        proc_macro2::Span::call_site(),
    );

    let register = if need_register {
        quote! {
            type MetricsProvider = #name;
            const _ASSERT_MAIN_GENERATED: () =
                {PROFILER_MAIN_GENERATED}; // const from bench_main
            // Mb compare name of function
            // const _ASSERT_IN_MAIN: () = {
            //     let caller = {
            //         fn f() {}
            //         let name = core::any::type_name_of_val(&f);
            //         &name[..name.len() - 3]
            //     };
            //     if caller.as_bytes() != b"main" {
            //         panic!("Metrics can only be registered in the main function");
            //     }
            // };
        }
    } else {
        quote!()
    };

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
                use #crate_path::SingleMetric;
                #(#end_calls)*
                (#(#result_tuple_fields,)*)
            }

            fn metrics_names() -> &'static [&'static str] {
                &[#(#field_names),*]
            }

            fn format_value(&self, metric_idx: usize, value: f64) -> (String, &'static str) {
                use #crate_path::SingleMetric;
                match metric_idx {
                    #(#format_match_arms,)*
                    _ => #crate_path::format_unit_helper(value),
                }
            }

            fn result_to_f64(&self, metric_idx: usize, result: &Self::Result) -> f64 {
                use #crate_path::SingleMetric;
                match metric_idx {
                    #(#result_to_f64_match_arms,)*
                    _ => panic!("Invalid metric index"),
                }
            }
        }

        #register
    };

    TokenStream::from(expanded)
}
