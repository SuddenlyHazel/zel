use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Lit, Token};

/// Parsed attributes from #[zel_service(name = "...")]
pub struct ServiceAttr {
    pub name: String,
}

impl Parse for ServiceAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;

        // Parse key = value pairs
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: Lit = input.parse()?;

            if key == "name" {
                if let Lit::Str(s) = value {
                    name = Some(s.value());
                } else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "Expected string literal for name",
                    ));
                }
            }

            // Allow trailing comma
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        name.ok_or_else(|| input.error("Missing 'name' attribute"))
            .map(|name| ServiceAttr { name })
    }
}

/// Method attribute: #[method(name = "...")]
#[derive(Debug, Clone)]
pub struct MethodAttr {
    pub name: String,
}

/// Subscription attribute: #[subscription(name = "...", item = "Type")]
#[derive(Debug, Clone)]
pub struct SubscriptionAttr {
    pub name: String,
    pub item: Option<syn::Type>,
}

/// Stream attribute: #[stream(name = "...")]
#[derive(Debug, Clone)]
pub struct StreamAttr {
    pub name: String,
}

/// Parse method or subscription attributes from a method
pub fn parse_method_attr(attrs: &[Attribute]) -> syn::Result<Option<MethodAttr>> {
    for attr in attrs {
        if attr.path().is_ident("method") {
            return attr
                .parse_args::<MethodAttrParser>()
                .map(|p| Some(p.into()));
        }
    }
    Ok(None)
}

pub fn parse_subscription_attr(attrs: &[Attribute]) -> syn::Result<Option<SubscriptionAttr>> {
    for attr in attrs {
        if attr.path().is_ident("subscription") {
            return attr
                .parse_args::<SubscriptionAttrParser>()
                .map(|p| Some(p.into()));
        }
    }
    Ok(None)
}

pub fn parse_stream_attr(attrs: &[Attribute]) -> syn::Result<Option<StreamAttr>> {
    for attr in attrs {
        if attr.path().is_ident("stream") {
            return attr
                .parse_args::<StreamAttrParser>()
                .map(|p| Some(p.into()));
        }
    }
    Ok(None)
}

struct MethodAttrParser {
    name: String,
}

impl Parse for MethodAttrParser {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;

        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: Lit = input.parse()?;

            if key == "name" {
                if let Lit::Str(s) = value {
                    name = Some(s.value());
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        name.ok_or_else(|| input.error("Missing 'name' attribute"))
            .map(|name| MethodAttrParser { name })
    }
}

impl From<MethodAttrParser> for MethodAttr {
    fn from(p: MethodAttrParser) -> Self {
        MethodAttr { name: p.name }
    }
}

struct SubscriptionAttrParser {
    name: String,
    item: Option<syn::Type>,
}

impl Parse for SubscriptionAttrParser {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut item = None;

        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            if key == "name" {
                let value: Lit = input.parse()?;
                if let Lit::Str(s) = value {
                    name = Some(s.value());
                }
            } else if key == "item" {
                let value: Lit = input.parse()?;
                if let Lit::Str(s) = value {
                    item = Some(syn::parse_str::<syn::Type>(&s.value())?);
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        name.ok_or_else(|| input.error("Missing 'name' attribute"))
            .map(|name| SubscriptionAttrParser { name, item })
    }
}

impl From<SubscriptionAttrParser> for SubscriptionAttr {
    fn from(p: SubscriptionAttrParser) -> Self {
        SubscriptionAttr {
            name: p.name,
            item: p.item,
        }
    }
}

struct StreamAttrParser {
    name: String,
}

impl Parse for StreamAttrParser {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;

        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: Lit = input.parse()?;

            if key == "name" {
                if let Lit::Str(s) = value {
                    name = Some(s.value());
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        name.ok_or_else(|| input.error("Missing 'name' attribute"))
            .map(|name| StreamAttrParser { name })
    }
}

impl From<StreamAttrParser> for StreamAttr {
    fn from(p: StreamAttrParser) -> Self {
        StreamAttr { name: p.name }
    }
}
