//! The `OrbitModel` derive for `orbit-orm`. It is re-exported from `orbit-orm`; use that path.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Error, Fields, Ident, LitStr, Result, Type, parse_macro_input};

mod naming;

/// Implements `FromRow` and `Model` for a struct with named fields.
#[proc_macro_derive(OrbitModel, attributes(orbit))]
pub fn derive_orbit_model(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// The struct-level `#[orbit(...)]` keys.
#[derive(Default)]
struct Options {
    table: Option<LitStr>,
    primary_key: Option<LitStr>,
}

/// One named field and the column it maps to.
struct Column {
    ident: Ident,
    ty: Type,
    name: String,
    skip: bool,
    fillable: bool,
}

fn expand(input: &DeriveInput) -> Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "OrbitModel does not support generic structs",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "OrbitModel can only be derived for a struct",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(Error::new_spanned(
            &input.ident,
            "OrbitModel needs a struct with named fields",
        ));
    };

    let options = struct_options(input)?;
    let columns = named.named.iter().map(column).collect::<Result<Vec<_>>>()?;

    // Without `table`, the name is Laravel's default: the plural of the struct name.
    let table = match options.table {
        Some(table) => {
            if table.value().is_empty() {
                return Err(Error::new_spanned(
                    &table,
                    "the table name must not be empty",
                ));
            }
            table
        }
        None => LitStr::new(
            &naming::table_name(&input.ident.unraw().to_string()),
            input.ident.span(),
        ),
    };

    let primary_key = match &options.primary_key {
        Some(key) => {
            let mapped = columns.iter().any(|c| !c.skip && c.name == key.value());
            if !mapped {
                return Err(Error::new_spanned(
                    key,
                    format!("no field maps to the primary key column `{}`", key.value()),
                ));
            }
            key.value()
        }
        None => "id".to_owned(),
    };

    let name = &input.ident;
    let fields = columns.iter().map(|c| {
        let ident = &c.ident;
        if c.skip {
            quote!(#ident: ::core::default::Default::default())
        } else {
            let column = &c.name;
            quote!(#ident: ::orbit_orm::__sqlx::Row::try_get(row, #column)?)
        }
    });
    let fillable = columns.iter().filter(|c| c.fillable).map(|c| &c.name);
    // Each mapped field must decode from the row's database, whichever one that is. This is the
    // bound that sqlx's own `FromRow` derive emits, so one impl serves every enabled driver.
    let decodes = columns.iter().filter(|c| !c.skip).map(|c| &c.ty);

    Ok(quote! {
        impl<'r, R> ::orbit_orm::__sqlx::FromRow<'r, R> for #name
        where
            R: ::orbit_orm::__sqlx::Row,
            &'r ::core::primitive::str: ::orbit_orm::__sqlx::ColumnIndex<R>,
            #(
                #decodes: ::orbit_orm::__sqlx::Decode<'r, <R as ::orbit_orm::__sqlx::Row>::Database>
                    + ::orbit_orm::__sqlx::Type<<R as ::orbit_orm::__sqlx::Row>::Database>,
            )*
        {
            fn from_row(row: &'r R) -> ::core::result::Result<Self, ::orbit_orm::__sqlx::Error> {
                ::core::result::Result::Ok(Self { #(#fields),* })
            }
        }

        impl ::orbit_orm::Model for #name {
            const TABLE: &'static str = #table;
            const PRIMARY_KEY: &'static str = #primary_key;
            const FILLABLE: &'static [&'static str] = &[#(#fillable),*];
        }
    })
}

/// Reads the struct-level `#[orbit(...)]` attributes.
fn struct_options(input: &DeriveInput) -> Result<Options> {
    let mut options = Options::default();
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("orbit")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("table") {
                options.table = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("primary_key") {
                options.primary_key = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error("unknown key; a struct takes `table` and `primary_key`"));
            }
            Ok(())
        })?;
    }
    Ok(options)
}

/// Reads one field's `#[orbit(...)]` attributes.
fn column(field: &syn::Field) -> Result<Column> {
    let ident = field.ident.clone().expect("named fields have idents");
    let mut name: Option<LitStr> = None;
    let mut skip: Option<Span> = None;
    let mut fillable: Option<Span> = None;

    for attr in field.attrs.iter().filter(|a| a.path().is_ident("orbit")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("column") {
                name = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("skip") {
                skip = Some(meta.path.span());
            } else if meta.path.is_ident("fillable") {
                fillable = Some(meta.path.span());
            } else {
                return Err(
                    meta.error("unknown key; a field takes `column`, `skip` and `fillable`")
                );
            }
            Ok(())
        })?;
    }

    if let Some(span) = skip {
        if let Some(name) = &name {
            return Err(Error::new_spanned(
                name,
                "`skip` cannot be combined with `column`",
            ));
        }
        if fillable.is_some() {
            return Err(Error::new(
                span,
                "`skip` cannot be combined with `fillable`",
            ));
        }
    }
    if let Some(name) = &name
        && name.value().is_empty()
    {
        return Err(Error::new_spanned(
            name,
            "the column name must not be empty",
        ));
    }

    let name = match &name {
        Some(name) => name.value(),
        None => ident.unraw().to_string(),
    };
    Ok(Column {
        ident,
        ty: field.ty.clone(),
        name,
        skip: skip.is_some(),
        fillable: fillable.is_some(),
    })
}
