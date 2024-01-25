use {
    indexmap::IndexMap,
    std::{collections::HashMap, iter, path::Path},
    wit_parser::{
        Docs, Function, FunctionKind, Interface, InterfaceId, Resolve, Result_, Results, Type,
        TypeDef, TypeDefKind, TypeId, TypeOwner, UnresolvedPackage, World, WorldId, WorldItem,
        WorldKey,
    },
};

struct Asyncify<'a> {
    old_resolve: &'a Resolve,
    new_resolve: Resolve,
    pending: TypeId,
    ready: TypeId,
    interfaces: HashMap<InterfaceId, InterfaceId>,
    functions: HashMap<WorldKey, (Function, Function)>,
}

impl<'a> Asyncify<'a> {
    fn asyncify_world_item(
        &mut self,
        key: &WorldKey,
        item: &WorldItem,
    ) -> Vec<(WorldKey, WorldItem)> {
        let mut new_key = || match key {
            WorldKey::Name(name) => WorldKey::Name(name.clone()),
            WorldKey::Interface(old) => {
                WorldKey::Interface(if let Some(new) = self.interfaces.get(old).copied() {
                    new
                } else {
                    let new = self.asyncify_interface(*old);
                    self.interfaces.insert(*old, new);
                    new
                })
            }
        };

        match item {
            WorldItem::Interface(old) => {
                vec![(
                    new_key(),
                    WorldItem::Interface(if let Some(new) = self.interfaces.get(old).copied() {
                        new
                    } else {
                        let new = self.asyncify_interface(*old);
                        self.interfaces.insert(*old, new);
                        new
                    }),
                )]
            }
            WorldItem::Function(old) => {
                let new_key = |suffix| match key {
                    WorldKey::Name(name) => WorldKey::Name(format!("{name}{suffix}")),
                    WorldKey::Interface(_) => unreachable!(),
                };

                let (a, b) = if let Some(new) = self.functions.get(key).cloned() {
                    new
                } else {
                    let new = self.asyncify_function(old);
                    self.functions.insert(key.clone(), new.clone());
                    new
                };
                vec![
                    (new_key("-isyswasfa"), WorldItem::Function(a)),
                    (new_key("-isyswasfa-result"), WorldItem::Function(b)),
                ]
            }
            WorldItem::Type(old) => vec![(new_key(), WorldItem::Type(*old))],
        }
    }

    fn asyncify_interface(&mut self, interface: InterfaceId) -> InterfaceId {
        let old = &self.old_resolve.interfaces[interface];
        let functions = old
            .functions
            .iter()
            .flat_map(|(_, function)| {
                let (a, b) = self.asyncify_function(function);
                [(a.name.clone(), a), (b.name.clone(), b)]
            })
            .collect();

        self.new_resolve.interfaces.alloc(Interface {
            name: old.name.as_ref().map(|s| format!("{s}-isyswasfa")),
            types: old.types.clone(),
            functions,
            docs: old.docs.clone(),
            package: old.package,
        })
    }

    fn asyncify_function(&mut self, function: &Function) -> (Function, Function) {
        (
            Function {
                name: format!("{}-isysasfa", function.name),
                kind: function.kind.clone(),
                params: function.params.clone(),
                results: match &function.results {
                    Results::Anon(ty) => {
                        Results::Anon(Type::Id(self.new_resolve.types.alloc(TypeDef {
                            name: None,
                            kind: TypeDefKind::Result(Result_ {
                                ok: Some(*ty),
                                err: Some(Type::Id(self.pending)),
                            }),
                            owner: TypeOwner::None,
                            docs: Docs::default(),
                        })))
                    }
                    Results::Named(_) => {
                        todo!("handle functions returning multiple named results")
                    }
                },
                docs: function.docs.clone(),
            },
            Function {
                name: format!("{}-isysasfa-result", function.name),
                kind: function.kind.clone(),
                params: vec![("pending".into(), Type::Id(self.ready))],
                results: function.results.clone(),
                docs: function.docs.clone(),
            },
        )
    }
}

pub fn asyncify(resolve: &Resolve, world: WorldId, poll_suffix: &str) -> (Resolve, WorldId) {
    let old_world = &resolve.worlds[world];

    let mut new_resolve = resolve.clone();

    let isyswasfa_package = new_resolve
        .push(
            UnresolvedPackage::parse(
                &Path::new("isyswasfa.wit"),
                include_str!("../../../../wit/isyswasfa.wit"),
            )
            .unwrap(),
        )
        .unwrap();

    let isyswasfa_interface = new_resolve.packages[isyswasfa_package].interfaces["isyswasfa"];

    let poll_input = new_resolve.interfaces[isyswasfa_interface].types["poll-input"];
    let list_poll_input = new_resolve.types.alloc(TypeDef {
        name: None,
        kind: TypeDefKind::List(Type::Id(poll_input)),
        owner: TypeOwner::None,
        docs: Docs::default(),
    });

    let poll_output = new_resolve.interfaces[isyswasfa_interface].types["poll-output"];
    let list_poll_output = new_resolve.types.alloc(TypeDef {
        name: None,
        kind: TypeDefKind::List(Type::Id(poll_output)),
        owner: TypeOwner::None,
        docs: Docs::default(),
    });

    let new_world = new_resolve.worlds.alloc(World {
        name: format!("{}-isyswasfa", old_world.name),
        imports: IndexMap::new(),
        exports: IndexMap::new(),
        package: old_world.package,
        docs: old_world.docs.clone(),
        includes: Vec::new(),
        include_names: Vec::new(),
    });

    let poll_function_name = format!("isyswasfa-poll-{poll_suffix}");
    let poll_function = Function {
        name: poll_function_name.clone(),
        kind: FunctionKind::Freestanding,
        params: vec![("input".to_owned(), Type::Id(list_poll_input))],
        results: Results::Anon(Type::Id(list_poll_output)),
        docs: Docs::default(),
    };

    let pending = new_resolve.interfaces[isyswasfa_interface].types["pending"];
    let ready = new_resolve.interfaces[isyswasfa_interface].types["ready"];

    let mut asyncify = Asyncify {
        old_resolve: resolve,
        new_resolve,
        pending,
        ready,
        interfaces: HashMap::new(),
        functions: HashMap::new(),
    };

    let imports = old_world
        .imports
        .iter()
        .flat_map(|(key, item)| asyncify.asyncify_world_item(key, item))
        .chain(iter::once((
            WorldKey::Interface(isyswasfa_interface),
            WorldItem::Interface(isyswasfa_interface),
        )))
        .collect();

    let exports = old_world
        .imports
        .iter()
        .flat_map(|(key, item)| asyncify.asyncify_world_item(key, item))
        .chain(iter::once((
            WorldKey::Name(poll_function_name),
            WorldItem::Function(poll_function),
        )))
        .collect();

    {
        let new_world = &mut asyncify.new_resolve.worlds[new_world];
        new_world.imports = imports;
        new_world.exports = exports;
    }

    (asyncify.new_resolve, new_world)
}
