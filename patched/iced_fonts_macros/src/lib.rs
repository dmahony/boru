use std::collections::HashMap;

use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use quote::quote;
use syn::{
    LitInt, LitStr,
    parse::{Parse, ParseStream},
    parse_macro_input,
    token::Comma,
};
use ttf_parser::Face;

struct Input {
    /// e.g. `"fonts/bootstrap-icons-new.ttf"`
    font_path: LitStr,
    /// e.g. `bootstrap`
    module_name: Ident,
    /// e.g. `"BOOTSTRAP_FONT"`
    font_name: Ident,
    /// e.g. `https://icons.getbootstrap.com/icons`
    doc_link: Option<LitStr>,
}

impl Parse for Input {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let font_path = input.parse()?;
        let _: Comma = input.parse()?;
        let module_name = input.parse()?;
        let _: Comma = input.parse()?;
        let font_name = input.parse()?;

        // It is good-mannered to accept an optional trailing comma
        let _: Option<Comma> = input.parse()?;
        let doc_link = input.parse()?;
        let _: Option<Comma> = input.parse()?;

        Ok(Self {
            font_path,
            module_name,
            font_name,
            doc_link,
        })
    }
}

/// Generates a module with functions that create text widgets.
#[proc_macro]
pub fn generate_icon_functions(input: TokenStream) -> TokenStream {
    body(input, "basic")
}

/// Generates a module with functions that create text widgets with advanced shaping.
#[proc_macro]
pub fn generate_icon_advanced_functions(input: TokenStream) -> TokenStream {
    body(input, "advanced")
}

fn body(input: TokenStream, shaping: &str) -> TokenStream {
    let Input {
        font_path,
        module_name,
        font_name,
        doc_link,
    } = parse_macro_input!(input as Input);

    let font_path_str = font_path.value();
    // Resolve relative font paths against CARGO_MANIFEST_DIR (the crate being
    // compiled), not the rustc working directory — cargo runs rustc for path
    // dependencies with cwd = workspace root, so a bare relative path like
    // "font.ttf" fails with NotFound for any patched/vendored crate. This keeps
    // iced_fonts and iced_aw (patched) building regardless of cwd.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let resolved_path = std::path::Path::new(&manifest_dir).join(&font_path_str);
    let font_data = std::fs::read(&resolved_path).expect("Failed to read font file");
    let face = Face::parse(&font_data, 0).expect("Failed to parse font");

    let mut all_codepoints: Vec<char> = Vec::new();
    if let Some(unicode_subtable) = face
        .tables()
        .cmap
        .unwrap()
        .subtables
        .into_iter()
        .find(|s| s.is_unicode())
    {
        unicode_subtable.codepoints(|c| {
            use std::convert::TryFrom;
            if let Ok(u) = char::try_from(c) {
                all_codepoints.push(u);
            }
        });
    }

    let mut functions = proc_macro2::TokenStream::new();
    let mut advanced_functions = proc_macro2::TokenStream::new();
    let mut duplicates: HashMap<String, u32> = HashMap::new();
    let mut count = 0;

    #[cfg(feature = "_generate_demo")]
    let mut demo_counter = 0;
    #[cfg(feature = "_generate_demo")]
    let mut demo_rows = 0;
    #[cfg(feature = "_generate_demo")]
    println!("row![");
    'outer: for c in all_codepoints {
        if let Some(glyph_id) = face.glyph_index(c) {
            let raw_name = face.glyph_name(glyph_id).unwrap_or("unnamed");

            // We need to rename some common characters.
            let mut processed_name = raw_name
                .replace("-", "_")
                .replace('0', "zero")
                .replace('1', "one")
                .replace('2', "two")
                .replace('3', "three")
                .replace('4', "four")
                .replace('5', "five")
                .replace('6', "six")
                .replace('7', "seven")
                .replace('8', "eight")
                .replace('9', "nine");

            // Material font edge case
            if processed_name.as_str() == "_" {
                processed_name = String::from("underscore");
            }

            // In case we have illegals. There are cases where most fonts have a .null icon that
            // doesn't do anything. So we can safely filter it out with the rest
            for c in processed_name.chars() {
                match c {
                    '+' | '-' | '*' | '/' | '@' | '!' | '#' | '$' | '%' | '^' | '&' | '(' | ')'
                    | '=' | '~' | '`' | ';' | ':' | '"' | '\'' | ',' | '<' | '>' | '?' | '.'
                    | ' ' | '[' | ']' | '{' | '}' | '|' | '\\' => continue 'outer,
                    _ => {}
                }
            }

            // Check for duplicates
            match duplicates.get(&processed_name) {
                Some(amount) => {
                    duplicates.insert(processed_name.clone(), *amount + 1);
                    // We don't care about repeats. Even though we should :(
                    continue 'outer;
                }
                None => {
                    duplicates.insert(processed_name.clone(), 1);
                }
            }

            #[cfg(feature = "_generate_demo")]
            if demo_rows < 18 {
                if demo_counter == 27 {
                    demo_counter = 0;
                    demo_rows += 1;

                    println!("{}(),", processed_name);
                    println!("]");
                    println!(".padding(12)");
                    println!(".spacing(20)");
                    println!(".width(Length::Fill)");
                    println!(".align_y(Center),");
                    println!("row![");
                } else {
                    demo_counter += 1;
                    println!("{}(),", processed_name);
                }
            }
            let fn_name = Ident::new_raw(&processed_name, Span::call_site());

            let doc = match doc_link {
                Some(ref location) => format!(
                    " Returns an [`iced_widget::Text`] widget of the [{} {}]({}/{}) icon.",
                    c,
                    processed_name,
                    location.value(),
                    raw_name,
                ),
                None => format!(
                    " Returns an [`iced_widget::Text`] widget of the {} {} icon.",
                    c, processed_name
                ),
            };

            let shaping = match shaping {
                "basic" => {
                    quote! { text::Shaping::Basic }
                }
                "advanced" => {
                    quote! { text::Shaping::Advanced }
                }
                _ => panic!(
                    "Shaping either needs to be basic or advanced, if you are unsure use advanced."
                ),
            };

            functions.extend(quote! {
                #[doc = #doc]
                #[must_use]
                pub fn #fn_name<'a, Theme: Catalog + 'a, Renderer: text::Renderer<Font = Font>>() -> Text<'a, Theme, Renderer> {
                    use iced_widget::text;
                    text(#c).font(#font_name).shaping(#shaping)
                }
            });

            let doc = format!(
                " Returns the [`String`] of {} character for lower level API's",
                processed_name
            );
            advanced_functions.extend(quote! {
                #[doc = #doc]
                #[must_use]
                pub fn #fn_name() -> (String, Font, Shaping) {
                    (#c.to_string(), #font_name, #shaping)
                }
            });

            count += 1;
        }
    }

    #[cfg(feature = "_generate_demo")]
    println!("We have {} icons", count);

    let advanced_text_tokens = if cfg!(feature = "advanced_text") {
        quote! {
          /// Every icon with helpers to use these icons in widgets.
          ///
          /// Usage
          /// ```
          /// let (content, font, shaping) = advanced_text::my_icon();
          ///
          /// advanced::Text {
          ///     content,
          ///     font,
          ///     shaping,
          ///     ...
          /// }
          /// ```
          pub mod advanced_text {
              use iced_widget::core::Font;
              use iced_widget::text::{self, Shaping};
              use crate::#font_name;

              #advanced_functions
          }
        }
    } else {
        quote! {}
    };

    let count_lit = LitInt::new(&count.to_string(), Span::call_site());
    let doc = format!(
        "A module with a function for every icon in {}'s font.",
        module_name.to_string()
    );
    TokenStream::from(quote! {
        #[doc = #doc]
        pub mod #module_name {
            use iced_widget::core::text;
            use iced_widget::core::Font;
            use iced_widget::text::Text;
            use iced_widget::text::Catalog;
            use crate::#font_name;

            /// The amount of icons in the font.
            pub const COUNT: usize = #count_lit;

            #functions

            #advanced_text_tokens

        }
    })
}
