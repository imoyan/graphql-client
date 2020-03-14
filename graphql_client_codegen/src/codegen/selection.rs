//! Code generation for the selection on an operation or a fragment.

use crate::codegen::decorate_type;
use crate::resolution::FragmentRef;
use crate::resolution::ResolvedFragmentId;
use crate::resolution::SelectedField;
use crate::resolution::SelectionRef;
use crate::schema::TypeRef;
use crate::shared::field_rename_annotation;
use crate::{
    field_type::GraphqlTypeQualifier,
    // deprecation::DeprecationStrategy,
    resolution::{OperationRef, ResolvedQuery, Selection, SelectionId},
    schema::{Schema, TypeId},
    shared::keyword_replace,
    GraphQLClientCodegenOptions,
};
use heck::*;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use std::borrow::Cow;

pub(crate) fn render_response_data_fields<'a>(
    operation: &OperationRef<'a>,
    response_derives: &impl quote::ToTokens,
    options: &GraphQLClientCodegenOptions,
) -> TokenStream {
    let mut expanded_selection = ExpandedSelection {
        query: operation.query(),
        schema: operation.schema(),
        types: Vec::with_capacity(8),
        variants: Vec::new(),
        fields: Vec::with_capacity(operation.selection_ids().len()),
        options,
    };

    let response_data_type_id = expanded_selection.push_type(ExpandedType {
        name: Cow::Borrowed("ResponseData"),
        schema_type: operation.on_ref(),
    });

    calculate_selection(
        &mut expanded_selection,
        operation.selection_ids(),
        response_data_type_id,
        operation.on_ref(),
    );

    expanded_selection.render(response_derives)
}

pub(super) fn render_fragment(
    fragment: &FragmentRef<'_>,
    response_derives: &impl quote::ToTokens,
    options: &GraphQLClientCodegenOptions,
) -> TokenStream {
    let mut expanded_selection = ExpandedSelection {
        query: fragment.query(),
        schema: fragment.schema(),
        types: Vec::with_capacity(8),
        variants: Vec::new(),
        fields: Vec::with_capacity(fragment.selection_ids().len()),
        options,
    };

    let response_type_id = expanded_selection.push_type(ExpandedType {
        name: fragment.name().into(),
        schema_type: fragment.on_ref(),
    });

    calculate_selection(
        &mut expanded_selection,
        fragment.selection_ids(),
        response_type_id,
        fragment.on_ref(),
    );

    expanded_selection.render(response_derives)
}

fn calculate_selection<'a>(
    context: &mut ExpandedSelection<'a>,
    selection_set: &[SelectionId],
    struct_id: ResponseTypeId,
    type_ref: TypeRef<'a>,
) {
    // TODO: if the selection has one item, we can sometimes generate fewer structs (e.g. single fragment spread)

    // If we are on a union or an interface, we need to generate an enum that matches the variants _exhaustively_,
    // including an `Other { #serde(rename = "__typename") typename: String }` variant.
    {
        let variants: Option<Cow<'_, [TypeId]>> = match type_ref.type_id() {
            TypeId::Interface(interface_id) => {
                let interface = context.schema().interface(interface_id);

                Some(interface.variants().collect())
            }
            TypeId::Union(union_id) => {
                let union = context.schema().union(union_id);
                Some(union.variants().into())
            }
            _ => None,
        };

        if let Some(variants) = variants {
            // for each variant, get the corresponding fragment spread, or default to an empty variant
            for variant in variants.as_ref() {
                let schema_type = context.schema().type_ref(*variant);
                let variant_name_str = schema_type.name();

                let selection = selection_set
                    .iter()
                    .map(|id| context.get_selection_ref(*id))
                    .filter_map(|selection_ref| {
                        selection_ref
                            .selection()
                            .as_inline_fragment()
                            .map(|inline_fragment| (selection_ref, inline_fragment))
                    })
                    .find(|(_selection_ref, inline_fragment)| inline_fragment.type_id == *variant);

                if let Some((selection_ref, inline_fragment)) = selection {
                    let variant_struct_name_str = selection_ref.full_path_prefix();

                    todo!("There will be a struct/type for the variant if there is an inline OR type-refining fragment there.");

                    context.push_variant(ExpandedVariant {
                        name: variant_name_str.into(),
                        variant_type: Some(variant_struct_name_str.clone().into()),
                        on: struct_id,
                    });

                    let expanded_type = ExpandedType {
                        name: variant_struct_name_str.into(),
                        schema_type,
                    };

                    let struct_id = context.push_type(expanded_type);

                    calculate_selection(
                        context,
                        selection_ref.subselection_ids(),
                        struct_id,
                        schema_type,
                    );
                } else {
                    context.push_variant(ExpandedVariant {
                        name: variant_name_str.into(),
                        on: struct_id,
                        variant_type: None,
                    });
                }
            }

            // push the fragments on variants down

            // meaning get all the fragment spreads on one of the variants, and add it to the type for that variant....
            todo!("push the fragments on variants down");

            // Finish by adding the Other variant
            todo!("add the Other variant");
        }
    }

    for id in selection_set {
        let selection_ref = context.get_selection_ref(*id);

        match selection_ref.selection() {
            Selection::Field(field) => {
                let (graphql_name, rust_name) = context.field_name(&field);
                let schema_field = field.schema_field(context.schema());
                let field_type = schema_field.field_type();

                match field_type.type_id() {
                    TypeId::Enum(enm) => {
                        context.push_field(ExpandedField {
                            graphql_name,
                            rust_name,
                            struct_id,
                            field_type: context.schema().r#enum(enm).name().into(),
                            field_type_qualifiers: schema_field.type_qualifiers(),
                            flatten: false,
                        });
                    }
                    TypeId::Scalar(scalar) => {
                        context.push_field(ExpandedField {
                            field_type: context.schema().scalar(scalar).name().into(),
                            field_type_qualifiers: field
                                .schema_field(context.schema())
                                .type_qualifiers(),
                            graphql_name,
                            struct_id,
                            rust_name,
                            flatten: false,
                        });
                    }
                    TypeId::Object(_) | TypeId::Interface(_) | TypeId::Union(_) => {
                        let struct_name_string = selection_ref.full_path_prefix();

                        context.push_field(ExpandedField {
                            struct_id,
                            graphql_name,
                            rust_name,
                            field_type_qualifiers: schema_field.type_qualifiers(),
                            field_type: Cow::Owned(struct_name_string.clone()),
                            flatten: false,
                        });

                        let type_id = context.push_type(ExpandedType {
                            name: Cow::Owned(struct_name_string),
                            schema_type: field_type,
                        });

                        calculate_selection(
                            context,
                            selection_ref.subselection_ids(),
                            type_id,
                            field_type,
                        );
                    }
                    TypeId::Input(_) => unreachable!("field selection on input type"),
                };
            }
            Selection::Typename => (),
            Selection::InlineFragment(_inline) => (),
            Selection::FragmentSpread(fragment_id) => {
                // FIXME: we need to identify if the fragment is on the field itself, or on an union/interface variant of it.
                // If it's on a field, do it here.
                // If it's on a variant, push it downstream to the variant.
                let fragment = context.get_fragment_ref(*fragment_id);

                // Assuming the query was validated properly, a fragment spread is either on the field's type itself, or on one of the variants (union or interfaces). If it's not directly a field on the struct, it will be handled in the `on` variants.
                if fragment.on() != type_ref.type_id() {
                    continue;
                }

                let original_field_name = fragment.name().to_snake_case();
                let final_field_name = keyword_replace(original_field_name);

                context.push_field(ExpandedField {
                    field_type: fragment.name().into(),
                    field_type_qualifiers: &[GraphqlTypeQualifier::Required],
                    graphql_name: fragment.name(),
                    rust_name: final_field_name,
                    struct_id,
                    flatten: true,
                });

                // We stop here, because the structs for the fragments are generated separately, to
                // avoid duplication.
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ResponseTypeId(u32);

struct ExpandedField<'a> {
    graphql_name: &'a str,
    rust_name: Cow<'a, str>,
    field_type: Cow<'a, str>,
    field_type_qualifiers: &'a [GraphqlTypeQualifier],
    struct_id: ResponseTypeId,
    flatten: bool,
}

impl<'a> ExpandedField<'a> {
    fn render(&self) -> TokenStream {
        let ident = Ident::new(&self.rust_name, Span::call_site());
        let qualified_type = decorate_type(
            &Ident::new(&self.field_type, Span::call_site()),
            self.field_type_qualifiers,
        );

        let optional_rename = field_rename_annotation(self.graphql_name, &self.rust_name);
        let optional_flatten = if self.flatten {
            Some(quote!(#[serde(flatten)]))
        } else {
            None
        };

        // TODO: deprecation
        // let deprecation_annotation = match (
        //     field.schema_field().is_deprecated(),
        //     options.deprecation_strategy(),
        // ) {
        //     (false, _) | (true, DeprecationStrategy::Allow) => None,
        //     (true, DeprecationStrategy::Warn) => {
        //         let msg = field
        //             .schema_field()
        //             .deprecation_message()
        //             .unwrap_or("This field is deprecated.");

        //         Some(quote!(#[deprecated(note = #msg)]))
        //     }
        //     (true, DeprecationStrategy::Deny) => continue,
        // };

        quote! {
            #optional_flatten
            #optional_rename
            pub #ident: #qualified_type
        }
    }
}

struct ExpandedVariant<'a> {
    name: Cow<'a, str>,
    variant_type: Option<Cow<'a, str>>,
    on: ResponseTypeId,
}

impl<'a> ExpandedVariant<'a> {
    fn render(&self) -> TokenStream {
        let name_ident = Ident::new(&self.name, Span::call_site());
        let optional_type_ident = self.variant_type.as_ref().map(|variant_type| {
            let ident = Ident::new(&variant_type, Span::call_site());
            quote!((#ident))
        });

        quote!(#name_ident #optional_type_ident)
    }
}

struct ExpandedType<'a> {
    name: Cow<'a, str>,
    schema_type: TypeRef<'a>,
}

struct ExpandedSelection<'a> {
    query: &'a ResolvedQuery,
    schema: &'a Schema,
    types: Vec<ExpandedType<'a>>,
    fields: Vec<ExpandedField<'a>>,
    variants: Vec<ExpandedVariant<'a>>,
    options: &'a GraphQLClientCodegenOptions,
}

impl<'a> ExpandedSelection<'a> {
    pub(crate) fn schema(&self) -> &'a Schema {
        self.schema
    }

    pub(crate) fn push_type(&mut self, tpe: ExpandedType<'a>) -> ResponseTypeId {
        let id = self.types.len();
        self.types.push(tpe);

        ResponseTypeId(id as u32)
    }

    pub(crate) fn push_field(&mut self, field: ExpandedField<'a>) {
        self.fields.push(field);
    }

    pub(crate) fn push_variant(&mut self, variant: ExpandedVariant<'a>) {
        self.variants.push(variant);
    }

    pub(crate) fn get_selection_ref(&self, selection_id: SelectionId) -> SelectionRef<'a> {
        self.query.get_selection_ref(self.schema, selection_id)
    }

    pub(crate) fn get_fragment_ref(&self, fragment_id: ResolvedFragmentId) -> FragmentRef<'a> {
        self.query.get_fragment_ref(self.schema, fragment_id)
    }

    /// Returns a tuple to be interpreted as (graphql_name, rust_name).
    pub(crate) fn field_name(&self, field: &'a SelectedField) -> (&'a str, Cow<'a, str>) {
        let name = field
            .alias()
            .unwrap_or_else(|| field.schema_field(self.schema).name());
        let snake_case_name = name.to_snake_case();
        let final_name = keyword_replace(snake_case_name);

        (name, final_name)
    }

    fn types(&self) -> impl Iterator<Item = (ResponseTypeId, &ExpandedType<'_>)> {
        self.types
            .iter()
            .enumerate()
            .map(|(idx, ty)| (ResponseTypeId(idx as u32), ty))
    }

    pub fn render(&self, response_derives: &impl quote::ToTokens) -> TokenStream {
        let mut items = Vec::with_capacity(self.types.len());

        for (type_id, ty) in self.types() {
            let struct_name = Ident::new(&ty.name, Span::call_site());
            let fields = self
                .fields
                .iter()
                .filter(|field| field.struct_id == type_id)
                .map(|field| field.render());

            let on_variants: Vec<TokenStream> = self
                .variants
                .iter()
                .filter(|variant| variant.on == type_id)
                .map(|variant| variant.render())
                .collect();

            let (on_field, on_enum) = if on_variants.len() > 0 {
                let enum_name = Ident::new(&format!("{}On", ty.name), Span::call_site());

                let on_field = quote!(pub on: #enum_name);

                let on_enum = quote!(
                    #response_derives
                    pub enum #enum_name {
                        #(#on_variants),*
                    }
                );

                (Some(on_field), Some(on_enum))
            } else {
                (None, None)
            };

            let tokens = quote! {
                #response_derives
                pub struct #struct_name {
                    #(#fields,)*
                    #on_field
                }

                #on_enum
            };

            items.push(tokens);
        }

        quote!(#(#items)*)
    }
}
