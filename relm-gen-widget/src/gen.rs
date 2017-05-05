/*
 * Copyright (c) 2017 Boucher, Antoni <bouanto@zoho.com>
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy of
 * this software and associated documentation files (the "Software"), to deal in
 * the Software without restriction, including without limitation the rights to
 * use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
 * the Software, and to permit persons to whom the Software is furnished to do so,
 * subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
 * FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
 * COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
 * IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
 * CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

use std::collections::HashMap;

use quote::Tokens;
use syn::{Generics, Ident, Path, parse_path};
use syn::fold::Folder;

use parser::{
    Event,
    GtkWidget,
    RelmWidget,
    Widget,
    RELM_WIDGET_CLONE_IDENT,
    RELM_WIDGET_SELF_IDENT,
};
use parser::EventValue::{CurrentWidget, ForeignWidget};
use parser::EventValueReturn::{CallReturn, Return, WithoutReturn};
use parser::EitherWidget::{Gtk, Relm};
use remover::Remover;
use super::{Driver, MODEL_IDENT};

use self::WidgetType::*;

macro_rules! gen_set_prop_calls {
    ($widget:expr, $ident:expr) => {{
        let ident = $ident;
        let mut properties = vec![];
        let mut visible_properties = vec![];
        for (key, value) in &$widget.properties {
            let mut remover = Remover::new();
            let new_value = remover.fold_expr(value.clone());
            let property_func = Ident::new(format!("set_{}", key));
            let property = quote! {
                #ident.#property_func(#new_value);
            };
            if key == "visible" {
                visible_properties.push(property);
            }
            else {
                properties.push(property);
            }
        }
        (properties, visible_properties)
    }};
}

macro_rules! set_container {
    ($_self:expr, $widget:expr, $widget_name:expr, $widget_type:expr) => {
        if let Some(ref container_type) = $widget.container_type {
            if $_self.container_names.contains_key(container_type) {
                let attribute =
                    if let Some(ref typ) = *container_type {
                        format!("#[container=\"{}\"]", typ)
                    }
                    else {
                        "#[container]".to_string()
                    };
                panic!("Cannot use the {} attribute twice in the same widget", attribute);
            }
            $_self.relm_widgets.insert($widget_name.clone(), $widget_type.clone());
            $_self.container_names.insert(container_type.clone(), ($widget_name.clone(), $widget_type.clone()));
        }
    };
}

#[derive(Clone, Copy, PartialEq)]
enum WidgetType {
    IsGtk,
    IsRelm,
}

pub fn gen(name: &Ident, widget: &Widget, driver: &mut Driver) -> (Tokens, HashMap<Ident, Path>, Tokens)
{
    let mut generator = Generator::new(driver);
    let widget_tokens = generator.widget(widget, None, IsGtk);
    let driver = generator.driver.take().expect("driver");
    let idents: Vec<_> = driver.widgets.keys().collect();
    let root_widget_name = &driver.root_widget.as_ref().expect("root_widget is None");
    let widget_names1: Vec<_> = generator.widget_names.iter()
        .filter(|ident| (idents.contains(ident) || generator.relm_widgets.contains_key(ident))
                && ident != root_widget_name)
        .collect();
    let widget_names1 = &widget_names1;
    let widget_names2 = widget_names1;
    let widget_names3 = widget_names1;
    let events = &generator.events;
    let self_ident = Ident::new(RELM_WIDGET_SELF_IDENT);
    let clone_ident = Ident::new(RELM_WIDGET_CLONE_IDENT);
    let model_ident = Ident::new(MODEL_IDENT);
    let code = quote! {
        #widget_tokens

        let #self_ident = ::std::rc::Rc::new(::std::cell::RefCell::new(#name {
            #root_widget_name: #root_widget_name,
            #(#widget_names1: #widget_names2,)*
            model: #model_ident,
        }));

        {
            let #clone_ident = ::std::rc::Rc::downgrade(&#self_ident);
            let #name { ref #root_widget_name, #(ref #widget_names3,)* .. } = *#self_ident.borrow();
            #(#events)*
        }

        #self_ident
    };
    let container_impl = gen_container_impl(&generator, widget, driver.generic_types.as_ref().expect("generic types"));
    (code, generator.relm_widgets, container_impl)
}

struct Generator<'a> {
    container_names: HashMap<Option<String>, (Ident, Path)>,
    driver: Option<&'a mut Driver>,
    events: Vec<Tokens>,
    relm_widgets: HashMap<Ident, Path>,
    widget_names: Vec<Ident>,
}

impl<'a> Generator<'a> {
    fn new(driver: &'a mut Driver) -> Self {
        Generator {
            container_names: HashMap::new(),
            driver: Some(driver),
            events: vec![],
            relm_widgets: HashMap::new(),
            widget_names: vec![],
        }
    }

    fn add_child_or_show_all(&mut self, widget: &Widget, parent: Option<&Ident>, parent_widget_type: WidgetType)
        -> Tokens
    {
        let widget_name = &widget.name;
        if let Some(name) = parent {
            if parent_widget_type == IsGtk {
                quote! {
                    ::gtk::ContainerExt::add(&#name, &#widget_name);
                }
            }
            else {
                quote! {
                    ::relm::RelmContainer::add(&#name, &#widget_name);
                }
            }
        }
        else {
            let struct_name = &widget.typ;
            let driver = self.driver.as_mut().expect("driver");
            driver.root_widget_type = Some(quote! {
                #struct_name
            });
            driver.root_widget = Some(widget_name.clone());
            driver.root_widget_expr = Some(quote! {
                #widget_name
            });
            quote! {
            }
        }
    }

    fn add_or_create_widget(&mut self, parent: Option<&Ident>, parent_widget_type: WidgetType, widget_name: &Ident,
        widget_type_ident: &Path, init_parameters: &[Tokens]) -> Tokens
    {
        let init_parameters = gen_model_param(init_parameters);
        if let Some(parent) = parent {
            if parent_widget_type == IsGtk {
                quote! {
                    let #widget_name = {
                        ::relm::ContainerWidget::add_widget::<#widget_type_ident, _>(&#parent, &relm,
                            #init_parameters)
                    };
                }
            }
            else {
                quote! {
                    let #widget_name = {
                        ::relm::RelmContainer::add_widget::<#widget_type_ident, _>(&#parent, &relm,
                            #init_parameters)
                    };
                }
            }
        }
        else {
            let driver = self.driver.as_mut().expect("driver");
            driver.root_widget_type = Some(quote! {
                <#widget_type_ident as ::relm::Widget>::Root
            });
            driver.root_widget = Some(widget_name.clone());
            driver.root_widget_expr = Some(quote! {
                #widget_name.widget().root()
            });
            quote! {
                let #widget_name = {
                    ::relm::create_component::<#widget_type_ident, _>(&relm, #init_parameters)
                };
            }
        }
    }

    fn collect_event(&mut self, widget_name: &Ident, save: bool, name: &str, event: &Event) {
        let event_ident = Ident::new(format!("connect_{}", name));
        let event_params: Vec<_> = event.params.iter().map(|ident| Ident::new(ident.as_ref())).collect();
        let event_model_ident = gen_model_ident(event);
        let clone = gen_clone(save);
        let connect =
            match event.value {
                CurrentWidget(WithoutReturn(ref event_value)) => quote! {{
                    #clone
                    connect!(relm, #widget_name, #event_ident(#(#event_params),*), #event_value);
                }},
                ForeignWidget(ref foreign_widget_name, WithoutReturn(ref event_value)) => quote! {{
                    #clone
                    connect!(#widget_name, #event_ident(#(#event_params),*), #foreign_widget_name, #event_value);
                }},
                CurrentWidget(Return(ref event_value, ref return_value)) => quote! {{
                    #clone
                    connect!(relm, #widget_name, #event_ident(#(#event_params),*) (#event_value, #return_value));
                }},
                ForeignWidget(_, Return(_, _)) | ForeignWidget(_, CallReturn(_)) => unreachable!(),
                CurrentWidget(CallReturn(ref func)) => quote! {{
                    #clone
                    connect!(relm, #widget_name, #event_ident(#(#event_params),*) #event_model_ident #func);
                }},

            };
        self.events.push(connect);
    }

    fn collect_events(&mut self, widget: &Widget, gtk_widget: &GtkWidget) {
        let widget_name = &widget.name;
        for (name, event) in &gtk_widget.events {
            self.collect_event(widget_name, gtk_widget.save, name, event);
        }
        for (&(ref child_name, ref name), event) in &gtk_widget.child_events {
            let widget_name = Ident::new(format!("{}.get_{}()", widget_name, child_name));
            self.collect_event(&widget_name, false, &name, event);
        }
    }

    fn collect_relm_events(&mut self, widget: &Widget, relm_widget: &RelmWidget) {
        let widget_name = &widget.name;
        for (name, widget_events) in &relm_widget.events {
            let event_ident = Ident::new(name.as_ref());
            for event in widget_events {
                let params =
                    if event.params.is_empty() {
                        quote! {}
                    }
                    else {
                        let event_params: Vec<_> = event.params.iter()
                            .map(|ident| Ident::new(ident.as_ref()))
                            .collect();
                        quote! {
                            (#(#event_params),*)
                        }
                    };
                let event_model_ident = gen_model_ident(event);
                let clone = gen_clone(true);
                let connect =
                    match event.value {
                        CurrentWidget(WithoutReturn(ref event_value)) => quote! {{
                            #clone
                            connect!(#widget_name@#event_ident #params, relm, #event_model_ident #event_value);
                        }},
                        ForeignWidget(ref foreign_widget_name, WithoutReturn(ref event_value)) => quote! {{
                            #clone
                            connect!(#widget_name@#event_ident #params, #foreign_widget_name,
                                     #event_model_ident #event_value);
                        }},
                        CurrentWidget(Return(_, _)) | CurrentWidget(CallReturn(_)) | ForeignWidget(_, Return(_, _)) |
                            ForeignWidget(_, CallReturn(_)) => unreachable!(),
                    };
                self.events.push(connect);
            }
        }
    }

    fn gtk_widget(&mut self, widget: &Widget, gtk_widget: &GtkWidget, parent: Option<&Ident>,
        parent_widget_type: WidgetType) -> Tokens
    {
        let struct_name = &widget.typ;
        let widget_name = &widget.name;
        set_container!(self, widget, widget_name, struct_name);
        self.widget_names.push(widget_name.clone());

        if gtk_widget.save {
            self.relm_widgets.insert(widget_name.clone(), struct_name.clone());
        }

        let construct_widget = gen_construct_widget(widget);
        self.collect_events(widget, gtk_widget);

        let children: Vec<_> = widget.children.iter()
            .map(|child| self.widget(child, Some(widget_name), IsGtk))
            .collect();

        let add_child_or_show_all = self.add_child_or_show_all(widget, parent, parent_widget_type);
        let ident = quote! { #widget_name };
        let (properties, visible_properties) = gen_set_prop_calls!(widget, ident);
        let child_properties = gen_set_child_prop_calls(widget, parent, parent_widget_type, IsGtk);

        quote! {
            let #widget_name: #struct_name = #construct_widget;
            #(#properties)*
            #(#children)*
            #add_child_or_show_all
            #widget_name.show();
            #(#visible_properties)*
            #(#child_properties)*
        }
    }

    fn relm_widget(&mut self, widget: &Widget, relm_widget: &RelmWidget, parent: Option<&Ident>,
        parent_widget_type: WidgetType) -> Tokens
    {
        self.widget_names.push(widget.name.clone());
        let widget_name = &widget.name;
        let widget_type_ident = &widget.typ;
        set_container!(self, widget, widget_name, widget_type_ident);
        let relm_component_type = gen_relm_component_type(widget_type_ident);
        self.relm_widgets.insert(widget.name.clone(), relm_component_type);

        self.collect_relm_events(widget, relm_widget);

        let children: Vec<_> = widget.children.iter()
            .map(|child| self.widget(child, Some(widget_name), IsRelm))
            .collect();
        let ident = quote! { #widget_name.widget_mut() };
        let (properties, visible_properties) = gen_set_prop_calls!(widget, ident);

        let add_or_create_widget = self.add_or_create_widget(
            parent, parent_widget_type, widget_name, widget_type_ident, &widget.init_parameters);
        let child_properties = gen_set_child_prop_calls(widget, parent, parent_widget_type, IsRelm);

        quote! {
            #add_or_create_widget
            #(#properties)*
            #(#visible_properties)*
            #(#children)*
            #(#child_properties)*
        }
    }

    fn widget(&mut self, widget: &Widget, parent: Option<&Ident>, parent_widget_type: WidgetType) -> Tokens {
        match widget.widget {
            Gtk(ref gtk_widget) => self.gtk_widget(widget, gtk_widget, parent, parent_widget_type),
            Relm(ref relm_widget) => self.relm_widget(widget, relm_widget, parent, parent_widget_type),
        }
    }
}

fn gen_construct_widget(widget: &Widget) -> Tokens {
    let struct_name = &widget.typ;

    let params = &widget.init_parameters;

    if widget.init_parameters.is_empty() {
        quote! {
            unsafe {
                use gtk::StaticType;
                use relm::{Downcast, FromGlibPtrNone, ToGlib};
                ::gtk::Widget::from_glib_none(::relm::g_object_new(#struct_name::static_type().to_glib(),
                #(#params,)* ::std::ptr::null() as *const i8) as *mut _)
                .downcast_unchecked()
            }
        }
    }
    else {
        quote! {
            #struct_name::new(#(#params),*)
        }
    }
}

fn gen_model_ident(event: &Event) -> Tokens {
    let clone_ident = Ident::new(RELM_WIDGET_CLONE_IDENT);
    if event.use_self {
        quote! {
            with #clone_ident
        }
    }
    else {
        quote! {
        }
    }
}

fn gen_clone(save: bool) -> Tokens {
    let clone_ident = Ident::new(RELM_WIDGET_CLONE_IDENT);
    if save {
        quote! {
            let #clone_ident = #clone_ident.clone();
        }
    }
    else {
        quote! {
        }
    }
}

fn gen_widget_type(widget: &Widget) -> Tokens {
    match widget.widget {
        Gtk(ref gtk_widget) => {
            let ident = gtk_widget.relm_name.as_ref().unwrap();
            quote! {
                #ident
            }
        },
        Relm(_) => {
            let path = &widget.typ;
            quote! {
                #path
            }
        },
    }
}

fn gen_add_widget_method(container_names: &HashMap<Option<String>, (Ident, Path)>) -> Tokens {
    if container_names.len() > 1 {
        let mut default_container = Tokens::new();
        let mut other_containers = Tokens::new();
        for (parent_id, &(ref name, ref typ)) in container_names {
            let first_type_part = &typ.segments.first().expect("first segment").ident;
            let (container_trait, upcast_container) =
                if first_type_part == "gtk" {
                    (quote! {
                        ::gtk::ContainerExt
                    }, quote! {
                        ::relm::Cast::upcast(self.#name.clone())
                    })
                }
                else {
                    (quote! {
                        ::relm::RelmContainer
                    }, quote! {
                        ::relm::Cast::upcast(self.#name.widget().root())
                    })
                };
            if parent_id.is_none() {
                default_container = quote! {
                    #container_trait::add(&self.#name, &widget.root());
                    #upcast_container
                };
            }
            else {
                if other_containers.as_str().is_empty() {
                    other_containers = quote! {
                        if WIDGET::parent_id() == Some(#parent_id) {
                            #container_trait::add(&self.#name, &widget.root());
                            #upcast_container
                        }
                    };
                }
                else {
                    other_containers = quote! {
                        #other_containers
                        else if WIDGET::parent_id() == Some(#parent_id) {
                            #container_trait::add(&self.#name, &widget.root());
                            #upcast_container
                        }
                    };
                }
            }
        }
        if !other_containers.as_str().is_empty() {
            default_container = quote! {
                else {
                    #default_container
                }
            };
        }
        quote! {
            fn add_widget<WIDGET: Widget>(&self, widget: &WIDGET) -> ::gtk::Container {
                #other_containers
                #default_container
            }
        }
    }
    else {
        quote! {
        }
    }
}

fn gen_container_impl(generator: &Generator, widget: &Widget, generic_types: &Generics) -> Tokens {
    let widget_type = gen_widget_type(widget);
    if generator.container_names.is_empty() {
        quote! {
        }
    }
    else if !generator.container_names.contains_key(&None) {
        panic!("Use of #[container=\"name\"] attribute without the default #[container].");
    }
    else {
        let mut container_type = None;
        for (ident, &(_, ref typ)) in &generator.container_names {
            if ident.is_none() {
                container_type = Some(typ);
            }
        }
        let typ = container_type.expect("container type");
        let &(ref name, _) = generator.container_names.get(&None).expect("default container");
        let add_widget_method = gen_add_widget_method(&generator.container_names);

        quote! {
            impl #generic_types ::relm::Container for #widget_type {
                type Container = #typ;

                fn container(&self) -> &Self::Container {
                    &self.#name
                }

                #add_widget_method
            }
        }
    }
}

fn gen_model_param(init_parameters: &[Tokens]) -> Tokens {
    quote! {
        (#(#init_parameters),*)
    }
}

fn gen_relm_component_type(name: &Path) -> Path {
    let tokens = quote! {
        ::relm::Component<#name>
    };
    parse_path(tokens.as_str()).expect("gen_relm_component_type is a Path")
}

fn gen_set_child_prop_calls(widget: &Widget, parent: Option<&Ident>, parent_widget_type: WidgetType,
    widget_type: WidgetType) -> Vec<Tokens>
{
    let widget_name = &widget.name;
    let mut child_properties = vec![];
    if let Some(parent) = parent {
        for (key, value) in &widget.child_properties {
            let property_func = Ident::new(format!("set_child_{}", key));
            let parent =
                if parent_widget_type == IsGtk {
                    quote! {
                        #parent
                    }
                }
                else {
                    quote! {
                        ::relm::Container::container(&*#parent.widget())
                    }
                };
            let call =
                if widget_type == IsGtk {
                    quote! {
                        #parent.#property_func(&#widget_name, #value);
                    }
                }
                else {
                    quote! {
                        #parent.#property_func(&#widget_name.widget().root(), #value);
                    }
                };
            child_properties.push(call);
        }
    }
    child_properties
}
