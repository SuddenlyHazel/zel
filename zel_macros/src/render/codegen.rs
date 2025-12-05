use crate::helpers::ParamInfo;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

/// Convert snake_case to CamelCase
pub fn capitalize_first(s: &str) -> String {
    s.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Generate code to deserialize parameters from request data
/// Used in server-side registration functions
pub fn generate_param_deserialization(params: &[ParamInfo]) -> TokenStream2 {
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let param_types: Vec<_> = params.iter().map(|p| &p.ty).collect();

    if params.is_empty() {
        quote! {
            // No parameters
        }
    } else if params.len() == 1 {
        let ident = &param_idents[0];
        let ty = &param_types[0];
        quote! {
            let #ident: #ty = serde_json::from_slice(data)
                .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                    format!("Failed to deserialize parameter: {}", e)
                ))?;
        }
    } else {
        quote! {
            let params: (#(#param_types),*) = serde_json::from_slice(data)
                .map_err(|e| zel_core::protocol::ResourceError::CallbackError(
                    format!("Failed to deserialize parameters: {}", e)
                ))?;
            let (#(#param_idents),*) = params;
        }
    }
}

/// Generate code to serialize parameters for client-side calls
/// Returns TokenStream for parameter serialization
pub fn generate_param_serialization(params: &[ParamInfo]) -> TokenStream2 {
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();

    if params.is_empty() {
        quote! {
            let body = bytes::Bytes::new();
        }
    } else if params.len() == 1 {
        let param = &param_idents[0];
        quote! {
            let body = serde_json::to_vec(&#param)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = bytes::Bytes::from(body);
        }
    } else {
        quote! {
            let params = (#(#param_idents),*);
            let body = serde_json::to_vec(&params)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = bytes::Bytes::from(body);
        }
    }
}

/// Generate code to serialize parameters for client-side calls with optional body
/// Used in subscriptions, notifications, and streams
pub fn generate_param_serialization_optional(params: &[ParamInfo]) -> TokenStream2 {
    let param_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();

    if params.is_empty() {
        quote! {
            let body = None;
        }
    } else if params.len() == 1 {
        let param = &param_idents[0];
        quote! {
            let param_body = serde_json::to_vec(&#param)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = Some(bytes::Bytes::from(param_body));
        }
    } else {
        quote! {
            let params = (#(#param_idents),*);
            let param_body = serde_json::to_vec(&params)
                .map_err(|e| zel_core::protocol::ClientError::Serialization(e))?;
            let body = Some(bytes::Bytes::from(param_body));
        }
    }
}

/// Generate a typed name for sinks, receivers, streams, and senders
/// Examples:
/// - generate_typed_name("Math", "add", "Sink") -> "MathAddSink"
/// - generate_typed_name("Math", "counter", "Stream") -> "MathCounterStream"
pub fn generate_typed_name(
    trait_name: &syn::Ident,
    method_name: &syn::Ident,
    suffix: &str,
) -> syn::Ident {
    quote::format_ident!(
        "{}{}{}",
        trait_name,
        capitalize_first(&method_name.to_string()),
        suffix
    )
}
