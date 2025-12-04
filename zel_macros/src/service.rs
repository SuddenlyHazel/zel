use crate::attributes::{
    parse_method_attr, parse_subscription_attr, MethodAttr, ServiceAttr, SubscriptionAttr,
};
use crate::helpers::{extract_params, extract_return_type, is_async, ParamInfo};
use syn::{ItemTrait, TraitItem, TraitItemFn};

#[derive(Debug)]
pub struct ServiceDescription {
    pub service_name: String,
    pub trait_ident: syn::Ident,
    pub methods: Vec<MethodDescription>,
    pub subscriptions: Vec<SubscriptionDescription>,
}

#[derive(Debug, Clone)]
pub struct MethodDescription {
    pub rpc_name: String,
    pub signature: TraitItemFn,
    pub params: Vec<ParamInfo>,
    pub return_type: Option<syn::Type>,
}

#[derive(Debug, Clone)]
pub struct SubscriptionDescription {
    pub rpc_name: String,
    pub signature: TraitItemFn,
    pub params: Vec<ParamInfo>,
    pub item_type: Option<syn::Type>,
}

impl ServiceDescription {
    pub fn from_item(attr: ServiceAttr, trait_def: ItemTrait) -> syn::Result<Self> {
        let mut methods = Vec::new();
        let mut subscriptions = Vec::new();

        // Validate trait has no associated types or constants
        for item in &trait_def.items {
            match item {
                TraitItem::Const(_) => {
                    return Err(syn::Error::new_spanned(
                        item,
                        "Associated constants not supported in service traits",
                    ));
                }
                TraitItem::Type(_) => {
                    return Err(syn::Error::new_spanned(
                        item,
                        "Associated types not supported in service traits",
                    ));
                }
                _ => {}
            }
        }

        // Process methods
        for item in trait_def.items {
            let TraitItem::Fn(method) = item else {
                return Err(syn::Error::new_spanned(
                    item,
                    "Only methods allowed in service trait",
                ));
            };

            // Check for #[method] or #[subscription] attribute
            if let Some(method_attr) = parse_method_attr(&method.attrs)? {
                methods.push(MethodDescription::from_trait_fn(method_attr, method)?);
            } else if let Some(sub_attr) = parse_subscription_attr(&method.attrs)? {
                subscriptions.push(SubscriptionDescription::from_trait_fn(sub_attr, method)?);
            } else {
                return Err(syn::Error::new_spanned(
                    method,
                    "Method must have #[method] or #[subscription] attribute",
                ));
            }
        }

        // Ensure at least one method or subscription
        if methods.is_empty() && subscriptions.is_empty() {
            return Err(syn::Error::new_spanned(
                trait_def.ident,
                "Service trait must have at least one method or subscription",
            ));
        }

        Ok(Self {
            service_name: attr.name,
            trait_ident: trait_def.ident,
            methods,
            subscriptions,
        })
    }

    pub fn render(&self) -> proc_macro2::TokenStream {
        crate::render::render_service(self)
    }
}

impl MethodDescription {
    fn from_trait_fn(attr: MethodAttr, mut method: TraitItemFn) -> syn::Result<Self> {
        // Validate method is async
        if !is_async(&method.sig) {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "RPC methods must be async",
            ));
        }

        // Extract parameters
        let params = extract_params(&method.sig)?;

        // Extract return type
        let return_type = extract_return_type(&method.sig)?;

        // Remove the attributes from the signature (we've processed them)
        method.attrs.retain(|attr| {
            !attr.path().is_ident("method") && !attr.path().is_ident("subscription")
        });

        Ok(Self {
            rpc_name: attr.name,
            signature: method,
            params,
            return_type,
        })
    }
}

impl SubscriptionDescription {
    fn from_trait_fn(attr: SubscriptionAttr, mut method: TraitItemFn) -> syn::Result<Self> {
        // Validate method is async
        if !is_async(&method.sig) {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "Subscription methods must be async",
            ));
        }

        // Extract parameters
        let params = extract_params(&method.sig)?;

        // Remove the attributes from the signature
        method.attrs.retain(|attr| {
            !attr.path().is_ident("method") && !attr.path().is_ident("subscription")
        });

        Ok(Self {
            rpc_name: attr.name,
            signature: method,
            params,
            item_type: attr.item,
        })
    }
}
