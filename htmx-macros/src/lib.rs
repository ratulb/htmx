use htmx_script::{Script, ToJs};
use manyhow::{bail, manyhow, Result};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use quote_use::{quote_spanned_use as quote_spanned, quote_use as quote};
use rstml::atoms::OpenTag;
use rstml::node::{
    AttributeValueExpr, KeyedAttribute, KeyedAttributeValue, Node, NodeAttribute, NodeBlock,
    NodeElement, NodeName,
};
use syn::spanned::Spanned;
use syn::{parse2, ExprPath, LitStr};

#[manyhow]
#[proc_macro]
pub fn htmx(input: TokenStream) -> Result {
    // https://github.com/rust-lang/rust-analyzer/issues/15572
    // let htmx = match (
    //     proc_macro_crate::crate_name("htmx"),
    //     std::env::var("CARGO_CRATE_NAME").as_deref(),
    // ) { (Ok(FoundCrate::Itself), Ok("htmx")) => quote!(crate),
    //   (Ok(FoundCrate::Name(name)), _) => { let ident = Ident::new(&name,
    //   Span::call_site()); quote!(::#ident) } _ => quote!(::htmx),
    // };

    let mut input = input.into_iter().peekable();

    let htmx = match input.peek() {
        Some(TokenTree::Ident(ident)) if ident == "crate" => {
            input.next();
            quote!(crate)
        }
        _ => quote!(::htmx),
    };

    let nodes = rstml::Parser::new(
        rstml::ParserConfig::new()
            .recover_block(true)
            .element_close_use_default_wildcard_ident(false)
            .raw_text_elements(["script"].into()),
    )
    // TODO parse_recoverable
    .parse_simple(input.collect::<TokenStream>())?
    .into_iter()
    .map(expand_node)
    .collect::<Result<Vec<TokenStream>>>()?;
    let mut nodes = nodes.into_iter().peekable();

    Ok(if nodes.peek().is_some() {
        quote! {
        #use #htmx::{ToHtml, Html, IntoHtmlElements};
        {
            use #htmx::native::*;
            let mut $htmx = Html::new();
            #(
                ToHtml::write_to_html(&#nodes, &mut $htmx);
            )*
            $htmx
        }}
    } else {
        quote!(#htmx::Html::new())
    })
}

fn expand_node(node: Node) -> Result {
    Ok(match node {
        Node::Comment(_) => todo!(),
        Node::Doctype(_) => todo!(),
        Node::Fragment(_) => todo!(),
        Node::Element(NodeElement {
            open_tag: OpenTag {
                name, attributes, ..
            },
            children,
            close_tag,
            ..
        }) => {
            let script = name.to_string() == "script";
            let name = name_to_struct(name)?;
            let attributes = attributes
                .into_iter()
                .map(|attribute| match attribute {
                    NodeAttribute::Block(_) => {
                        bail!(attribute, "dynamic attribute names not supported")
                    }
                    NodeAttribute::Attribute(KeyedAttribute {
                        key,
                        possible_value,
                    }) => match possible_value {
                        KeyedAttributeValue::Binding(_) => todo!(),
                        KeyedAttributeValue::Value(AttributeValueExpr { value, .. }) => {
                            attribute_key_to_fn(key, value)
                        }
                        KeyedAttributeValue::None => attribute_key_to_fn(key, true),
                    },
                })
                .collect::<Result<Vec<_>>>()?;
            let children = if children.is_empty() {
                quote!()
            } else if script {
                let Some(Node::RawText(script)) = children.first() else {
                    unreachable!("script always raw text")
                };
                let script = script.into_token_stream();
                if let Ok(script) = parse2::<LitStr>(script.clone()) {
                    quote!(.child(#script))
                } else if let Ok(block) = parse2::<NodeBlock>(script.clone()) {
                    quote!( .child({#[allow(unused_braces)] #block}))
                } else {
                    let script: Script = parse2(script)?;
                    let script = script.to_java_script();
                    quote!(.child(#script))
                }
            } else {
                let children = children
                    .into_iter()
                    .map(expand_node)
                    .collect::<Result<Vec<_>>>()?;
                quote!(#(.child(&#children))*)
            };
            let main = quote!(#name::builder() #(.#attributes)* #children .build());
            match close_tag.map(|tag| name_to_struct(tag.name)) {
                // If close_tag was specified, use it so coloring happens
                Some(Ok(close_tag)) if close_tag == name => quote!({let _ :#close_tag;#main}),
                _ => main,
            }
        }
        Node::Block(_) | Node::Text(_) => quote!( {#[allow(unused_braces)] #node}),
        Node::RawText(_) => todo!(),
    })
}

fn name_to_struct(name: NodeName) -> Result<ExprPath> {
    match name {
        NodeName::Path(path) => Ok(path),
        // This {...}
        NodeName::Punctuated(_) | NodeName::Block(_) => {
            bail!(name, "Only normal identifiers are allowd as node names")
        }
    }
}

fn attribute_key_to_fn(name: NodeName, value: impl ToTokens) -> Result {
    match name {
        NodeName::Path(ExprPath { path, .. }) => Ok({
            let sident = path
                .segments
                .iter()
                .map(|i| i.ident.to_string().replace('_', "-"))
                .collect::<Vec<_>>()
                .join("-");
            if let Some(sident) = sident.strip_prefix("data-") {
                quote_spanned!(path.span()=> data(#sident, #value))
            } else if sident.starts_with("hx-") {
                quote_spanned!(path.span()=> data(#sident, #value))
            } else if let Some(ident) = path.get_ident() {
                quote!(#ident(#value))
            } else {
                bail!(path, "only `data::` or `hx::` are allowed as path prefix");
            }
        }),
        // This {...}
        NodeName::Punctuated(_) => {
            todo!("handle data-...")
        }
        NodeName::Block(_) => {
            bail!(
                name,
                "Only normal identifiers are allowd as attribute names"
            )
        }
    }
}

// todo derive macro
// #[component]
// fn MyFnComponent(a: bool, b: String) -> Html {
//     htmx! {crate
//         <button disabled=a> {b} </button>
//     }
// }
//
// // Generates
//
// #[derive(typed_builder::TypedBuilder)]
// #[builder(crate_module_path=::typed_builder)]
// #[builder(build_method(into = Html))]
// struct MyFnComponent {
//     a: bool,
//     b: String,
// }
//
// impl Into<Html> for MyFnComponent {
//     fn into(self) -> Html {
//         let Self { a, b } = self;
//         htmx! {crate
//             <button disabled=a> {b} </button>
//         }
//     }
// }
//
// // Using only struct
// #[derive(Component)]
// struct MyStructComponent {
//     a: bool,
//     b: String,
// }
// impl Into<Html> for MyStructComponent {
//     fn into(self) -> Html {
//         let Self { a, b } = self;
//         htmx! {crate
//             <button disabled=a> {b} </button>
//         }
//     }
// }
//
// // Generate
// #[derive(typed_builder::TypedBuilder)]
// #[builder(crate_module_path=::typed_builder)]
// #[builder(build_method(into = Html))]
//
