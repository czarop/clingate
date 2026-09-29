//! Transient messages, over the top of whatever tab is in front.
//!
//! Every report of something that just happened - a file written, a rule
//! saved, a run finished, a path that did not resolve - goes through here.
//! They used to be inline notes beside the button that caused them, which had
//! two faults: the note stayed until something else replaced it, so the editor
//! accumulated stale claims like "Loaded 150 gates from ..." long after the
//! fact; and a note could only be seen on the tab that wrote it, so a run
//! finishing while you were looking at the plots said nothing at all.
//!
//! State that *describes the form right now* - which rule is being edited, how
//! a pairing column resolves, how far through a run we are - is not a message
//! and stays inline. It has to be readable while you act on it, and a toast
//! that fades after five seconds would take it away mid-thought.
//!
//! ## Why every piece is wrapped
//!
//! The primitives carry behaviour and no appearance: the provider's container,
//! each toast and each of its parts take their class from the caller. Passing
//! the props straight through - which is what the other wrappers in this
//! directory do, because their primitives style themselves - renders the whole
//! thing unclassed, which means transparent text on a transparent background.
//! It is there in the document and invisible on screen. So this file mirrors
//! the upstream reference wrapper: a class on the container, and a
//! `render_toast` that builds a fully classed toast.

use dioxus::prelude::*;
use dioxus_primitives::toast::{
    self, Toast, ToastCloseButtonProps, ToastContentProps, ToastDescriptionProps, ToastProps,
    ToastPropsWithOwner, ToastTitleProps,
};
use std::time::Duration;

pub use dioxus_primitives::toast::{ToastOptions, Toasts, use_toast};

#[component]
fn StyledToast(props: ToastProps) -> Element {
    rsx! {
        Toast {
            id: props.id,
            index: props.index,
            title: props.title,
            description: props.description,
            toast_type: props.toast_type,
            on_close: props.on_close,
            permanent: props.permanent,
            duration: props.duration,
            class: "dx-toast",
            attributes: props.attributes,
            StyledContent {
                StyledTitle {}
                StyledDescription {}
            }
            StyledClose {}
        }
    }
}

#[component]
fn StyledContent(props: ToastContentProps) -> Element {
    rsx! {
        toast::ToastContent {
            class: "dx-toast-content",
            attributes: props.attributes,
            {props.children}
        }
    }
}

#[component]
fn StyledTitle(props: ToastTitleProps) -> Element {
    rsx! {
        toast::ToastTitle {
            class: "dx-toast-title",
            attributes: props.attributes,
            children: props.children,
        }
    }
}

#[component]
fn StyledDescription(props: ToastDescriptionProps) -> Element {
    rsx! {
        toast::ToastDescription {
            class: "dx-toast-description",
            attributes: props.attributes,
            children: props.children,
        }
    }
}

#[component]
fn StyledClose(props: ToastCloseButtonProps) -> Element {
    rsx! {
        toast::ToastCloseButton {
            class: "dx-toast-close",
            attributes: props.attributes,
            children: props.children,
        }
    }
}

/// Wraps the tree that may raise toasts, and draws them.
#[component]
pub fn ToastProvider(
    #[props(default = ReadSignal::new(Signal::new(Some(Duration::from_secs(5)))))]
    default_duration: ReadSignal<Option<Duration>>,
    #[props(default = ReadSignal::new(Signal::new(6)))] max_toasts: ReadSignal<usize>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: asset!("./style.css") }
        toast::ToastProvider {
            class: "dx-toast-container",
            default_duration,
            max_toasts,
            render_toast: Callback::new(|p: ToastPropsWithOwner| rsx! { StyledToast { ..p } }),
            attributes,
            {children}
        }
    }
}

/// How long a message stays up, by how much there is to read.
///
/// An error is worth longer than a confirmation: it usually names a path or a
/// parse failure, and it is the one a person actually needs to finish reading.
pub const GOOD: Duration = Duration::from_secs(4);
pub const BAD: Duration = Duration::from_secs(9);

/// Report success.
pub fn say(toasts: &Toasts, message: impl Into<String>) {
    toasts.success(message.into(), ToastOptions::new().duration(GOOD));
}

/// Report a failure the person can do something about.
pub fn warn(toasts: &Toasts, message: impl Into<String>) {
    toasts.error(message.into(), ToastOptions::new().duration(BAD));
}

/// Report something that happened without being asked for, or that needs
/// noticing but is not a failure.
pub fn note(toasts: &Toasts, message: impl Into<String>) {
    toasts.info(message.into(), ToastOptions::new().duration(GOOD));
}
