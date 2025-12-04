use crate::attributes::{
    parse_method_attr, parse_notification_attr, parse_stream_attr, parse_subscription_attr,
    MethodAttr, NotificationAttr, ServiceAttr, StreamAttr, SubscriptionAttr,
};
use crate::helpers::{extract_params, extract_return_type, is_async, ParamInfo};
use syn::{ItemTrait, TraitItem, TraitItemFn};

#[derive(Debug)]
pub struct ServiceDescription {
    pub service_name: String,
    pub trait_ident: syn::Ident,
    pub methods: Vec<MethodDescription>,
    pub subscriptions: Vec<SubscriptionDescription>,
    pub notifications: Vec<NotificationDescription>,
    pub streams: Vec<StreamDescription>,
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

#[derive(Debug, Clone)]
pub struct NotificationDescription {
    pub rpc_name: String,
    pub signature: TraitItemFn,
    pub params: Vec<ParamInfo>,
    pub item_type: Option<syn::Type>,
}

#[derive(Debug, Clone)]
pub struct StreamDescription {
    pub rpc_name: String,
    pub signature: TraitItemFn,
    pub params: Vec<ParamInfo>,
}

impl ServiceDescription {
    pub fn from_item(attr: ServiceAttr, trait_def: ItemTrait) -> syn::Result<Self> {
        let mut methods = Vec::new();
        let mut subscriptions = Vec::new();
        let mut notifications = Vec::new();
        let mut streams = Vec::new();

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

            // Check for #[method], #[subscription], #[notification], or #[stream] attribute
            if let Some(method_attr) = parse_method_attr(&method.attrs)? {
                methods.push(MethodDescription::from_trait_fn(method_attr, method)?);
            } else if let Some(sub_attr) = parse_subscription_attr(&method.attrs)? {
                subscriptions.push(SubscriptionDescription::from_trait_fn(sub_attr, method)?);
            } else if let Some(notif_attr) = parse_notification_attr(&method.attrs)? {
                notifications.push(NotificationDescription::from_trait_fn(notif_attr, method)?);
            } else if let Some(stream_attr) = parse_stream_attr(&method.attrs)? {
                streams.push(StreamDescription::from_trait_fn(stream_attr, method)?);
            } else {
                return Err(syn::Error::new_spanned(
                    method,
                    "Method must have #[method], #[subscription], #[notification], or #[stream] attribute",
                ));
            }
        }

        // Ensure at least one method, subscription, notification, or stream
        if methods.is_empty()
            && subscriptions.is_empty()
            && notifications.is_empty()
            && streams.is_empty()
        {
            return Err(syn::Error::new_spanned(
                trait_def.ident,
                "Service trait must have at least one method, subscription, notification, or stream",
            ));
        }

        Ok(Self {
            service_name: attr.name,
            trait_ident: trait_def.ident,
            methods,
            subscriptions,
            notifications,
            streams,
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

impl NotificationDescription {
    fn from_trait_fn(attr: NotificationAttr, mut method: TraitItemFn) -> syn::Result<Self> {
        // Validate method is async
        if !is_async(&method.sig) {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "Notification methods must be async",
            ));
        }

        // Extract parameters
        let params = extract_params(&method.sig)?;

        // Remove the attributes from the signature
        method.attrs.retain(|attr| {
            !attr.path().is_ident("method")
                && !attr.path().is_ident("subscription")
                && !attr.path().is_ident("notification")
        });

        Ok(Self {
            rpc_name: attr.name,
            signature: method,
            params,
            item_type: attr.item,
        })
    }
}

impl StreamDescription {
    fn from_trait_fn(attr: StreamAttr, mut method: TraitItemFn) -> syn::Result<Self> {
        // Validate method is async
        if !is_async(&method.sig) {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "Stream methods must be async",
            ));
        }

        // Extract user parameters (macro will inject ctx, send, recv later)
        let params = extract_params(&method.sig)?;

        // Remove the attributes from the signature
        method.attrs.retain(|attr| {
            !attr.path().is_ident("method")
                && !attr.path().is_ident("subscription")
                && !attr.path().is_ident("notification")
                && !attr.path().is_ident("stream")
        });

        Ok(Self {
            rpc_name: attr.name,
            signature: method,
            params,
        })
    }
}
