//! Development-only TypeScript export; no runtime dispatch or validation lives here.

use std::any::TypeId;
use std::collections::{BTreeMap, HashSet};
use ts_rs::{Config, TS, TypeVisitor};

use super::{
    protocol::{Push, RemoteError},
    view,
};

pub struct Bindings {
    config: Config,
    seen: HashSet<TypeId>,
    declarations: BTreeMap<String, String>,
    methods: BTreeMap<String, String>,
    commands: BTreeMap<String, String>,
}

impl TypeVisitor for Bindings {
    fn visit<T: TS + ?Sized + 'static>(&mut self) {
        if !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        if T::output_path().is_some() {
            let name = T::ident(&self.config);
            let declaration = T::decl(&self.config);
            if let Some(previous) = self.declarations.insert(name.clone(), declaration.clone()) {
                assert_eq!(previous, declaration, "conflicting TS name {name}");
            }
        }
        T::visit_dependencies(self);
        T::visit_generics(self);
    }
}

impl Bindings {
    fn ty<T: TS + 'static>(&mut self) -> String {
        self.visit::<T>();
        T::name(&self.config)
    }

    pub(super) fn method<P: TS + 'static, R: TS + 'static>(&mut self, name: &str) {
        let (params, result) = (self.ty::<P>(), self.ty::<R>());
        self.methods.insert(name.into(), format!("{name}: {{ params: {params}; result: {result} }};"));
    }

    pub(super) fn action<P: TS + 'static, R: TS + 'static>(&mut self, name: &str) {
        let (params, result) = (self.ty::<P>(), self.ty::<R>());
        self.commands.insert(name.into(), format!("{name}: {{ params: {params}; result: {result} }};"));
    }

    pub(super) fn command<C: lobotomy_core::Command + TS + 'static>(&mut self)
    where
        C::Output: TS + 'static,
    {
        self.action::<C, C::Output>(C::NAME);
    }
}

pub fn declarations() -> String {
    let mut bindings = Bindings {
        config: Config::new().with_large_int("number"),
        seen: HashSet::new(),
        declarations: BTreeMap::new(),
        methods: BTreeMap::new(),
        commands: BTreeMap::new(),
    };
    view::bindings(&mut bindings);
    super::command::bindings(&mut bindings);
    bindings.visit::<lobotomy_core::report::OrgReport>();
    bindings.visit::<lobotomy_core::report::ReportEffect>();
    bindings.visit::<Push>();
    bindings.visit::<RemoteError>();
    bindings.visit::<lobotomy_core::item::BlobRef>();
    let mut output: String =
        bindings.declarations.values().map(|declaration| format!("export {declaration}\n\n")).collect();
    for (name, entries) in [("ReadMethods", bindings.methods), ("Commands", bindings.commands)] {
        output.push_str(&format!("export type {name} = {{\n"));
        for entry in entries.values() {
            output.push_str(&format!("  {entry}\n"));
        }
        output.push_str("};\n\n");
    }
    output
}

pub fn fixtures() -> serde_json::Value {
    view::binding_fixtures()
}
