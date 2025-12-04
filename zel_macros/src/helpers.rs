use syn::{FnArg, Pat, PatType, Type};

/// Extract parameter information from a function signature
pub fn extract_params(sig: &syn::Signature) -> syn::Result<Vec<ParamInfo>> {
    let mut params = Vec::new();

    for input in &sig.inputs {
        match input {
            FnArg::Receiver(_) => {
                // Skip &self
                continue;
            }
            FnArg::Typed(PatType { pat, ty, .. }) => {
                if let Pat::Ident(pat_ident) = &**pat {
                    params.push(ParamInfo {
                        ident: pat_ident.ident.clone(),
                        ty: (**ty).clone(),
                    });
                } else {
                    return Err(syn::Error::new_spanned(
                        pat,
                        "Complex patterns not supported in parameters",
                    ));
                }
            }
        }
    }

    Ok(params)
}

#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub ident: syn::Ident,
    pub ty: Type,
}

/// Extract return type from function signature
pub fn extract_return_type(sig: &syn::Signature) -> syn::Result<Option<Type>> {
    match &sig.output {
        syn::ReturnType::Default => Ok(None),
        syn::ReturnType::Type(_, ty) => {
            // Extract inner type from Result<T, E>
            if let Type::Path(type_path) = &**ty {
                if let Some(segment) = type_path.path.segments.last() {
                    if segment.ident == "Result" {
                        if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                            if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                                return Ok(Some(inner_ty.clone()));
                            }
                        }
                    }
                }
            }
            Ok(Some((**ty).clone()))
        }
    }
}

/// Check if a method is async
pub fn is_async(sig: &syn::Signature) -> bool {
    sig.asyncness.is_some()
}
