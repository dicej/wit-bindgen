mod component_type_object;

use anyhow::Result;
use heck::{ToLowerCamelCase, ToShoutySnakeCase, ToUpperCamelCase};
use indexmap::IndexMap;
use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
    iter, mem,
    ops::Deref,
};
use wit_bindgen_core::{
    abi::{self, AbiVariant, Bindgen, Bitcast, Instruction, LiftLower, WasmType},
    wit_parser::LiveTypes,
    Direction,
};
use wit_bindgen_core::{
    uwrite, uwriteln,
    wit_parser::{
        Docs, Enum, Flags, FlagsRepr, Function, FunctionKind, Handle, Int, InterfaceId, Record,
        Resolve, Result_, SizeAlign, Tuple, Type, TypeDefKind, TypeId, TypeOwner, Variant, WorldId,
        WorldKey,
    },
    Files, InterfaceGenerator as _, Ns, WorldGenerator,
};
use wit_component::StringEncoding;
mod csproj;
pub use csproj::CSProject;

//TODO remove unused
const CSHARP_IMPORTS: &str = "\
using System;
using System.Runtime.CompilerServices;
using System.Collections;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
using System.Diagnostics;
";

#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Opts {
    /// Whether or not to generate a stub class for exported functions
    #[cfg_attr(feature = "clap", arg(long, default_value_t = StringEncoding::default()))]
    pub string_encoding: StringEncoding,
    #[cfg_attr(feature = "clap", arg(long))]
    pub generate_stub: bool,

    // TODO: This should only temporarily needed until mono and native aot aligns.
    #[cfg_attr(feature = "clap", arg(short, long, value_enum))]
    pub runtime: CSharpRuntime,
}

impl Opts {
    pub fn build(&self) -> Box<dyn WorldGenerator> {
        Box::new(CSharp {
            opts: self.clone(),
            ..CSharp::default()
        })
    }
}

struct ResourceInfo {
    name: String,
    docs: Docs,
    direction: Direction,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum CSharpRuntime {
    #[default]
    NativeAOT,
    Mono,
}

struct InterfaceFragment {
    csharp_src: String,
    csharp_interop_src: String,
    stub: String,
}

pub struct InterfaceTypeAndFragments {
    is_export: bool,
    interface_fragments: Vec<InterfaceFragment>,
}

impl InterfaceTypeAndFragments {
    pub fn new(is_export: bool) -> Self {
        InterfaceTypeAndFragments {
            is_export: is_export,
            interface_fragments: Vec::<InterfaceFragment>::new(),
        }
    }
}

/// Indicates if we are generating for functions in an interface or free standing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionLevel {
    Interface,
    FreeStanding,
}

#[derive(Default)]
pub struct CSharp {
    opts: Opts,
    name: String,
    return_area_size: usize,
    return_area_align: usize,
    tuple_counts: HashSet<usize>,
    needs_result: bool,
    needs_option: bool,
    needs_interop_string: bool,
    needs_export_return_area: bool,
    needs_rep_table: bool,
    interface_fragments: HashMap<String, InterfaceTypeAndFragments>,
    world_fragments: Vec<InterfaceFragment>,
    sizes: SizeAlign,
    interface_names: HashMap<InterfaceId, String>,
    anonymous_type_owners: HashMap<TypeId, TypeOwner>,
    resources: HashMap<TypeId, ResourceInfo>,
}

impl CSharp {
    fn qualifier(&self) -> String {
        let world = self.name.to_upper_camel_case();
        format!("{world}World.")
    }

    fn interface<'a>(
        &'a mut self,
        resolve: &'a Resolve,
        name: &'a str,
        direction: Direction,
        function_level: FunctionLevel,
    ) -> InterfaceGenerator<'a> {
        InterfaceGenerator {
            src: String::new(),
            csharp_interop_src: String::new(),
            stub: String::new(),
            gen: self,
            resolve,
            name,
            direction,
            function_level,
        }
    }

    // returns the qualifier and last part
    fn get_class_name_from_qualified_name(qualified_type: String) -> (String, String) {
        let parts: Vec<&str> = qualified_type.split('.').collect();
        if let Some(last_part) = parts.last() {
            let mut qualifier = qualified_type.strip_suffix(last_part);
            if qualifier.is_some() {
                qualifier = qualifier.unwrap().strip_suffix(".");
            }
            (qualifier.unwrap_or("").to_string(), last_part.to_string())
        } else {
            (String::new(), String::new())
        }
    }
}

impl WorldGenerator for CSharp {
    fn preprocess(&mut self, resolve: &Resolve, world: WorldId) {
        let name = &resolve.worlds[world].name;
        self.name = name.to_string();
        self.sizes.fill(resolve);
    }

    fn import_interface(
        &mut self,
        resolve: &Resolve,
        key: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) {
        let name = interface_name(self, resolve, key, Direction::Import);
        self.interface_names.insert(id, name.clone());
        let mut gen = self.interface(resolve, &name, Direction::Import, FunctionLevel::Interface);

        gen.types(id);

        for (resource, funcs) in by_resource(
            resolve.interfaces[id]
                .functions
                .iter()
                .map(|(k, v)| (k.as_str(), v)),
        ) {
            let import_module_name = &resolve.name_world_key(key);
            if let Some(resource) = resource {
                gen.start_resource(import_module_name, resource, "", &funcs);
            }

            for func in funcs {
                gen.import(import_module_name, func);
            }

            if resource.is_some() {
                gen.end_resource();
            }
        }

        // for anonymous types
        gen.define_interface_types(id);

        gen.add_interface_fragment(false);
    }

    fn import_funcs(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) {
        let name = &format!("{}-world", resolve.worlds[world].name).to_upper_camel_case();
        let name = &format!("{name}.I{name}");
        let mut gen = self.interface(
            resolve,
            name,
            Direction::Import,
            FunctionLevel::FreeStanding,
        );

        for (import_module_name, func) in funcs {
            gen.import(import_module_name, func);
        }

        gen.add_world_fragment();
    }

    fn export_interface(
        &mut self,
        resolve: &Resolve,
        key: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let name = interface_name(self, resolve, key, Direction::Export);
        self.interface_names.insert(id, name.clone());
        let mut gen = self.interface(resolve, &name, Direction::Export, FunctionLevel::Interface);

        gen.types(id);

        for (resource, funcs) in by_resource(
            resolve.interfaces[id]
                .functions
                .iter()
                .map(|(k, v)| (k.as_str(), v)),
        ) {
            if let Some(resource) = resource {
                gen.start_resource(
                    &format!("[export]{}", resolve.name_world_key(key)),
                    resource,
                    "abstract",
                    &funcs,
                );
            }

            for func in funcs {
                gen.export(func, Some(key));
            }

            if resource.is_some() {
                gen.end_resource();
            }
        }

        // for anonymous types
        gen.define_interface_types(id);

        gen.add_interface_fragment(true);
        Ok(())
    }

    fn export_funcs(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) -> Result<()> {
        let name = &format!("{}-world", resolve.worlds[world].name).to_upper_camel_case();
        let name = &format!("{name}.I{name}");
        let mut gen = self.interface(
            resolve,
            name,
            Direction::Export,
            FunctionLevel::FreeStanding,
        );

        for (resource, funcs) in by_resource(funcs.iter().copied()) {
            if let Some(resource) = resource {
                gen.start_resource("[export]$root", resource, "abstract", &funcs);
            }

            for func in funcs {
                gen.export(func, None);
            }

            if resource.is_some() {
                gen.end_resource();
            }
        }

        gen.add_world_fragment();
        Ok(())
    }

    fn import_types(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        types: &[(&str, TypeId)],
        _files: &mut Files,
    ) {
        let name = &format!("{}-world", resolve.worlds[world].name);
        let mut gen = self.interface(resolve, name, Direction::Import, FunctionLevel::Interface);

        for (ty_name, ty) in types {
            gen.define_type(ty_name, *ty);
        }

        gen.add_world_fragment();
    }

    fn finish(&mut self, resolve: &Resolve, id: WorldId, files: &mut Files) -> Result<()> {
        let world = &resolve.worlds[id];
        let world_namespace = self.qualifier();
        let world_namespace = world_namespace.strip_suffix(".").unwrap();
        let namespace = format!("{world_namespace}");
        let name = world.name.to_upper_camel_case();

        let version = env!("CARGO_PKG_VERSION");
        let mut src = String::new();
        uwriteln!(src, "// Generated by `wit-bindgen` {version}. DO NOT EDIT!");

        uwrite!(
            src,
            "{CSHARP_IMPORTS}

            namespace {world_namespace} {{

             public interface I{name}World {{
            "
        );

        src.push_str(
            &self
                .world_fragments
                .iter()
                .map(|f| f.csharp_src.deref())
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let mut producers = wasm_metadata::Producers::empty();
        producers.add(
            "processed-by",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        );

        src.push_str("}\n");

        if self.needs_result {
            src.push_str(
                r#"

                public readonly struct None {}

                [StructLayout(LayoutKind.Sequential)]
                public readonly struct Result<Ok, Err>
                {
                    public readonly byte Tag;
                    private readonly object value;

                    private Result(byte tag, object value)
                    {
                        Tag = tag;
                        this.value = value;
                    }

                    public static Result<Ok, Err> ok(Ok ok)
                    {
                        return new Result<Ok, Err>(OK, ok!);
                    }

                    public static Result<Ok, Err> err(Err err)
                    {
                        return new Result<Ok, Err>(ERR, err!);
                    }

                    public bool IsOk => Tag == OK;
                    public bool IsErr => Tag == ERR;

                    public Ok AsOk
                    {
                        get
                        {
                            if (Tag == OK) 
                                return (Ok)value;
                            else 
                                throw new ArgumentException("expected OK, got " + Tag);
                        }
                    }

                    public Err AsErr
                    {
                        get
                        {
                            if (Tag == ERR)
                                return (Err)value;
                            else
                                throw new ArgumentException("expected ERR, got " + Tag);
                        }
                    }

                    public const byte OK = 0;
                    public const byte ERR = 1;
                }
                "#,
            )
        }

        if self.needs_option {
            src.push_str(
                r#"

                public class Option<T> {
                    private static Option<T> none = new ();
                    
                    private Option()
                    {
                        HasValue = false;
                    }
                    
                    public Option(T v)
                    {
                        HasValue = true;
                        Value = v;
                    }
                    
                    public static Option<T> None => none;
                    
                    public bool HasValue { get; }
                    
                    public T? Value { get; }
                }
                "#,
            )
        }

        if self.needs_interop_string {
            src.push_str(
                r#"
                public static class InteropString
                {
                    internal static IntPtr FromString(string input, out int length)
                    {
                        var utf8Bytes = Encoding.UTF8.GetBytes(input);
                        length = utf8Bytes.Length;
                        var gcHandle = GCHandle.Alloc(utf8Bytes, GCHandleType.Pinned);
                        return gcHandle.AddrOfPinnedObject();
                    }
                }
                "#,
            )
        }

        // Declare a statically-allocated return area, if needed. We only do
        // this for export bindings, because import bindings allocate their
        // return-area on the stack.
        if self.needs_export_return_area {
            let mut ret_area_str = String::new();

            uwrite!(
                ret_area_str,
                "
                public static class InteropReturnArea
                {{
                    [InlineArray({0})]
                    [StructLayout(LayoutKind.Sequential, Pack = {1})]
                    internal struct ReturnArea
                    {{
                        private byte buffer;

                        internal unsafe int AddressOfReturnArea()
                        {{
                            fixed(byte* ptr = &buffer)
                            {{
                                return (int)ptr;
                            }}
                        }}
                    }}

                    [ThreadStatic]
                    internal static ReturnArea returnArea = default;
                }}
                ",
                self.return_area_size,
                self.return_area_align,
            );

            src.push_str(&ret_area_str);
        }

        if self.needs_rep_table {
            src.push_str("\n");
            src.push_str(include_str!("rep_table.cs"));
        }

        if !&self.world_fragments.is_empty() {
            src.push_str("\n");

            src.push_str("namespace exports {\n");
            src.push_str(&format!("public static class {name}World\n"));
            src.push_str("{");

            for fragement in &self.world_fragments {
                src.push_str("\n");

                src.push_str(&fragement.csharp_interop_src);
            }
            src.push_str("}\n");
            src.push_str("}\n");
        }

        src.push_str("\n");

        src.push_str("}\n");

        files.push(&format!("{name}.cs"), indent(&src).as_bytes());

        let mut cabi_relloc_src = String::new();

        cabi_relloc_src.push_str(
            r#"
                #include <stdlib.h>

                /* Done in C so we can avoid initializing the dotnet runtime and hence WASI libc */
                /* It would be preferable to do this in C# but the constrainst of cabi_realloc and the demands */
                /* of WASI libc prevent us doing so. */
                /* See https://github.com/bytecodealliance/wit-bindgen/issues/777  */
                /* and https://github.com/WebAssembly/wasi-libc/issues/452 */
                /* The component model `start` function might be an alternative to this depending on whether it */
                /* has the same constraints as `cabi_realloc` */
                __attribute__((__weak__, __export_name__("cabi_realloc")))
                void *cabi_realloc(void *ptr, size_t old_size, size_t align, size_t new_size) {
                    (void) old_size;
                    if (new_size == 0) return (void*) align;
                    void *ret = realloc(ptr, new_size);
                    if (!ret) abort();
                    return ret;
                }
            "#,
        );
        files.push(
            &format!("{name}World_cabi_realloc.c"),
            indent(&cabi_relloc_src).as_bytes(),
        );

        let generate_stub = |name: String, files: &mut Files, stubs: Stubs| {
            let (stub_namespace, interface_or_class_name) =
                CSharp::get_class_name_from_qualified_name(name.clone());

            let stub_class_name = format!(
                "{}Impl",
                match interface_or_class_name.starts_with("I") {
                    true => interface_or_class_name
                        .strip_prefix("I")
                        .unwrap()
                        .to_string(),
                    false => interface_or_class_name.clone(),
                }
            );

            let stub_file_name = match stub_namespace.len() {
                0 => stub_class_name.clone(),
                _ => format!("{stub_namespace}.{stub_class_name}"),
            };

            let (fragments, fully_qualified_namespace) = match stubs {
                Stubs::World(fragments) => {
                    let fully_qualified_namespace = format!("{namespace}");
                    (fragments, fully_qualified_namespace)
                }
                Stubs::Interface(fragments) => {
                    let fully_qualified_namespace = format!("{stub_namespace}");
                    (fragments, fully_qualified_namespace)
                }
            };

            let body = fragments
                .iter()
                .map(|f| f.stub.deref())
                .collect::<Vec<_>>()
                .join("\n");

            let body = format!(
                "// Generated by `wit-bindgen` {version}. DO NOT EDIT!
                {CSHARP_IMPORTS}

                namespace {fully_qualified_namespace};

                 public partial class {stub_class_name} : {interface_or_class_name} {{
                    {body}
                 }}
                "
            );

            files.push(&format!("{stub_file_name}.cs"), indent(&body).as_bytes());
        };

        if self.opts.generate_stub {
            generate_stub(
                format!("I{name}World"),
                files,
                Stubs::World(&self.world_fragments),
            );
        }

        //TODO: This is currently neede for mono even if it's built as a library.
        if self.opts.runtime == CSharpRuntime::Mono {
            files.push(
                &format!("MonoEntrypoint.cs",),
                indent(
                    r#"
                public class MonoEntrypoint() {
                    public static void Main() {
                    }
                }
                "#,
                )
                .as_bytes(),
            );
        }

        files.push(
            &format!("{world_namespace}_component_type.o",),
            component_type_object::object(resolve, id, self.opts.string_encoding)
                .unwrap()
                .as_slice(),
        );

        // TODO: remove when we switch to dotnet 9
        let mut wasm_import_linakge_src = String::new();

        wasm_import_linakge_src.push_str(
            r#"
            // temporarily add this attribute until it is available in dotnet 9
            namespace System.Runtime.InteropServices
            {
                internal partial class WasmImportLinkageAttribute : Attribute {}
            }
            "#,
        );
        files.push(
            &format!("{world_namespace}_wasm_import_linkage_attribute.cs"),
            indent(&wasm_import_linakge_src).as_bytes(),
        );

        for (full_name, interface_type_and_fragments) in &self.interface_fragments {
            let fragments = &interface_type_and_fragments.interface_fragments;

            let (namespace, interface_name) =
                &CSharp::get_class_name_from_qualified_name(full_name.to_string());

            // C#
            let body = fragments
                .iter()
                .map(|f| f.csharp_src.deref())
                .collect::<Vec<_>>()
                .join("\n");

            if body.len() > 0 {
                let body = format!(
                    "// Generated by `wit-bindgen` {version}. DO NOT EDIT!
                    {CSHARP_IMPORTS}

                    namespace {namespace};

                    public interface {interface_name} {{
                        {body}
                    }}
                    "
                );

                files.push(&format!("{full_name}.cs"), indent(&body).as_bytes());
            }

            // C# Interop
            let body = fragments
                .iter()
                .map(|f| f.csharp_interop_src.deref())
                .collect::<Vec<_>>()
                .join("\n");

            let class_name = interface_name.strip_prefix("I").unwrap();
            let body = format!(
                "// Generated by `wit-bindgen` {version}. DO NOT EDIT!
                {CSHARP_IMPORTS}

                namespace {namespace}
                {{
                  public static class {class_name}Interop {{
                      {body}
                  }}
                }}
                "
            );

            files.push(
                &format!("{namespace}.{class_name}Interop.cs"),
                indent(&body).as_bytes(),
            );

            if interface_type_and_fragments.is_export && self.opts.generate_stub {
                generate_stub(full_name.to_string(), files, Stubs::Interface(fragments));
            }
        }

        Ok(())
    }
}

struct InterfaceGenerator<'a> {
    src: String,
    csharp_interop_src: String,
    stub: String,
    gen: &'a mut CSharp,
    resolve: &'a Resolve,
    name: &'a str,
    direction: Direction,
    function_level: FunctionLevel,
}

impl InterfaceGenerator<'_> {
    fn define_interface_types(&mut self, id: InterfaceId) {
        let mut live = LiveTypes::default();
        live.add_interface(self.resolve, id);
        self.define_live_types(live, id);
    }

    //TODO: we probably need this for anonymous types outside of an interface...
    // fn define_function_types(&mut self, funcs: &[(&str, &Function)]) {
    //     let mut live = LiveTypes::default();
    //     for (_, func) in funcs {
    //         live.add_func(self.resolve, func);
    //     }
    //     self.define_live_types(live);
    // }

    fn define_live_types(&mut self, live: LiveTypes, id: InterfaceId) {
        let mut type_names = HashMap::new();

        for ty in live.iter() {
            // just create c# types for wit anonymous types
            let type_def = &self.resolve.types[ty];
            if type_names.contains_key(&ty) || type_def.name.is_some() {
                continue;
            }

            let typedef_name = self.type_name(&Type::Id(ty));

            let prev = type_names.insert(ty, typedef_name.clone());
            assert!(prev.is_none());

            // workaround for owner not set on anonymous types, maintain or own map to the owner
            self.gen
                .anonymous_type_owners
                .insert(ty, TypeOwner::Interface(id));

            self.define_anonymous_type(ty, &typedef_name)
        }
    }

    fn define_anonymous_type(&mut self, type_id: TypeId, typedef_name: &str) {
        let type_def = &self.resolve().types[type_id];
        let kind = &type_def.kind;

        // TODO Does c# need this exit?
        // // skip `typedef handle_x handle_y` where `handle_x` is the same as `handle_y`
        // if let TypeDefKind::Handle(handle) = kind {
        //     let resource = match handle {
        //         Handle::Borrow(id) | Handle::Own(id) => id,
        //     };
        //     let origin = dealias(self.resolve, *resource);
        //     if origin == *resource {
        //         return;
        //     }
        // }

        //TODO: what other TypeDefKind do we need here?
        match kind {
            TypeDefKind::Tuple(t) => self.type_tuple(type_id, typedef_name, t, &type_def.docs),
            TypeDefKind::Option(t) => self.type_option(type_id, typedef_name, t, &type_def.docs),
            TypeDefKind::Record(t) => self.type_record(type_id, typedef_name, t, &type_def.docs),
            TypeDefKind::List(t) => self.type_list(type_id, typedef_name, t, &type_def.docs),
            TypeDefKind::Variant(t) => self.type_variant(type_id, typedef_name, t, &type_def.docs),
            TypeDefKind::Result(t) => self.type_result(type_id, typedef_name, t, &type_def.docs),
            TypeDefKind::Handle(_) => {
                // TODO: Ensure we emit a type for each imported and exported resource, regardless of whether they
                // contain functions.
            }
            _ => unreachable!(),
        }
    }

    fn qualifier(&self, when: bool, ty: &TypeId) -> String {
        // anonymous types dont get an owner from wit-parser, so assume they are part of an interface here.
        let owner = if let Some(owner_type) = self.gen.anonymous_type_owners.get(ty) {
            *owner_type
        } else {
            let type_def = &self.resolve.types[*ty];
            type_def.owner
        };

        let global_prefix = self.global_if_user_type(&Type::Id(*ty));

        if let TypeOwner::Interface(id) = owner {
            if let Some(name) = self.gen.interface_names.get(&id) {
                if name != self.name {
                    return format!("{global_prefix}{name}.");
                }
            }
        }

        if when {
            let name = self.name;
            format!("{global_prefix}{name}.")
        } else {
            String::new()
        }
    }

    fn add_interface_fragment(self, is_export: bool) {
        self.gen
            .interface_fragments
            .entry(self.name.to_string())
            .or_insert_with(|| InterfaceTypeAndFragments::new(is_export))
            .interface_fragments
            .push(InterfaceFragment {
                csharp_src: self.src,
                csharp_interop_src: self.csharp_interop_src,
                stub: self.stub,
            });
    }

    fn add_world_fragment(self) {
        self.gen.world_fragments.push(InterfaceFragment {
            csharp_src: self.src,
            csharp_interop_src: self.csharp_interop_src,
            stub: self.stub,
        });
    }

    fn import(&mut self, import_module_name: &str, func: &Function) {
        let (camel_name, static_) = match &func.kind {
            FunctionKind::Freestanding | FunctionKind::Static(_) => {
                (func.item_name().to_upper_camel_case(), "static ")
            }
            FunctionKind::Method(_) => (func.item_name().to_upper_camel_case(), ""),
            FunctionKind::Constructor(id) => {
                (self.gen.resources[id].name.to_upper_camel_case(), "")
            }
        };

        let interop_camel_name = func.item_name().to_upper_camel_case();

        let sig = self.resolve.wasm_signature(AbiVariant::GuestImport, func);

        let wasm_result_type = match &sig.results[..] {
            [] => "void",
            [result] => wasm_type(*result),
            _ => unreachable!(),
        };

        let result_type = if let FunctionKind::Constructor(_) = &func.kind {
            String::new()
        } else {
            match func.results.len() {
                0 => "void".to_string(),
                1 => {
                    let ty = func.results.iter_types().next().unwrap();
                    self.type_name_with_qualifier(ty, true)
                }
                _ => {
                    let types = func
                        .results
                        .iter_types()
                        .map(|ty| self.type_name_with_qualifier(ty, true))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({})", types)
                }
            }
        };

        let wasm_params = sig
            .params
            .iter()
            .enumerate()
            .map(|(i, param)| {
                let ty = wasm_type(*param);
                format!("{ty} p{i}")
            })
            .collect::<Vec<_>>()
            .join(", ");

        let mut bindgen = FunctionBindgen::new(
            self,
            &func.item_name(),
            &func.kind,
            func.params
                .iter()
                .enumerate()
                .map(|(i, (name, _))| {
                    if i == 0 && matches!(&func.kind, FunctionKind::Method(_)) {
                        "this".to_owned()
                    } else {
                        name.to_csharp_ident()
                    }
                })
                .collect(),
        );

        abi::call(
            bindgen.gen.resolve,
            AbiVariant::GuestImport,
            LiftLower::LowerArgsLiftResults,
            func,
            &mut bindgen,
        );

        let src = bindgen.src;
        let import_return_pointer_area_size = bindgen.import_return_pointer_area_size;
        let import_return_pointer_area_align = bindgen.import_return_pointer_area_align;

        let params = func
            .params
            .iter()
            .skip(if let FunctionKind::Method(_) = &func.kind {
                1
            } else {
                0
            })
            .map(|param| {
                let ty = self.type_name_with_qualifier(&param.1, true);
                let param_name = &param.0;
                let param_name = param_name.to_csharp_ident();
                format!("{ty} {param_name}")
            })
            .collect::<Vec<_>>()
            .join(", ");

        let import_name = &func.name;

        let target = if let FunctionKind::Freestanding = &func.kind {
            &mut self.csharp_interop_src
        } else {
            &mut self.src
        };

        uwrite!(
            target,
            r#"
            internal static class {interop_camel_name}WasmInterop
            {{
                [DllImport("{import_module_name}", EntryPoint = "{import_name}"), WasmImportLinkage]
                internal static extern {wasm_result_type} wasmImport{interop_camel_name}({wasm_params});
            "#
        );

        if import_return_pointer_area_size > 0 {
            uwrite!(
                target,
                r#"
                [InlineArray({import_return_pointer_area_size})]
                [StructLayout(LayoutKind.Sequential, Pack = {import_return_pointer_area_align})]
                internal struct ImportReturnArea
                {{
                    private byte buffer;

                    internal unsafe int AddressOfReturnArea()
                    {{
                        fixed(byte* ptr = &buffer)
                        {{
                            return (int)ptr;
                        }}
                    }}
                }}
                "#,
            )
        }

        uwrite!(
            target,
            r#"
            }}
            "#,
        );

        uwrite!(
            target,
            r#"
                internal {static_}unsafe {result_type} {camel_name}({params})
                {{
                    {src}
                    //TODO: free alloc handle (interopString) if exists
                }}
            "#
        );
    }

    fn export(&mut self, func: &Function, interface_name: Option<&WorldKey>) {
        let (camel_name, modifiers) = match &func.kind {
            FunctionKind::Freestanding | FunctionKind::Static(_) => {
                (func.item_name().to_upper_camel_case(), "static ")
            }
            FunctionKind::Method(_) => (func.item_name().to_upper_camel_case(), "public "),
            FunctionKind::Constructor(id) => {
                (self.gen.resources[id].name.to_upper_camel_case(), "")
            }
        };

        let sig = self.resolve.wasm_signature(AbiVariant::GuestExport, func);

        let mut bindgen = FunctionBindgen::new(
            self,
            &func.item_name(),
            &func.kind,
            (0..sig.params.len()).map(|i| format!("p{i}")).collect(),
        );

        abi::call(
            bindgen.gen.resolve,
            AbiVariant::GuestExport,
            LiftLower::LiftArgsLowerResults,
            func,
            &mut bindgen,
        );

        assert!(!bindgen.needs_cleanup_list);

        let src = bindgen.src;

        let wasm_result_type = match &sig.results[..] {
            [] => "void",
            [result] => wasm_type(*result),
            _ => unreachable!(),
        };

        let result_type = if let FunctionKind::Constructor(_) = &func.kind {
            String::new()
        } else {
            match func.results.len() {
                0 => "void".to_owned(),
                1 => self.type_name(func.results.iter_types().next().unwrap()),
                _ => {
                    let types = func
                        .results
                        .iter_types()
                        .map(|ty| self.type_name(ty))
                        .collect::<Vec<String>>()
                        .join(", ");
                    format!("({}) ", types)
                }
            }
        };

        let wasm_params = sig
            .params
            .iter()
            .enumerate()
            .map(|(i, param)| {
                let ty = wasm_type(*param);
                format!("{ty} p{i}")
            })
            .collect::<Vec<_>>()
            .join(", ");

        let params = func
            .params
            .iter()
            .skip(if let FunctionKind::Method(_) = &func.kind {
                1
            } else {
                0
            })
            .map(|(name, ty)| {
                let ty = self.type_name(ty);
                let name = name.to_csharp_ident();
                format!("{ty} {name}")
            })
            .collect::<Vec<String>>()
            .join(", ");

        let interop_name = format!("wasmExport{camel_name}");
        let core_module_name = interface_name.map(|s| self.resolve.name_world_key(s));
        let export_name = func.core_export_name(core_module_name.as_deref());

        uwrite!(
            self.csharp_interop_src,
            r#"
            [UnmanagedCallersOnly(EntryPoint = "{export_name}")]
            public static unsafe {wasm_result_type} {interop_name}({wasm_params}) {{
                {src}
            }}
            "#
        );

        if !sig.results.is_empty() {
            uwrite!(
                self.csharp_interop_src,
                r#"
                [UnmanagedCallersOnly(EntryPoint = "cabi_post_{export_name}")]
                public static void cabi_post_{interop_name}({wasm_result_type} returnValue) {{
                    Console.WriteLine("cabi_post_{export_name}");
                }}
                "#
            );
        }

        if !matches!(
            &func.kind,
            FunctionKind::Constructor(_) | FunctionKind::Static(_)
        ) {
            uwrite!(
                self.src,
                r#"{modifiers}abstract {result_type} {camel_name}({params});

            "#
            );
        }

        if self.gen.opts.generate_stub {
            let sig = self.sig_string(func, true);

            uwrite!(
                self.stub,
                r#"
                {sig} {{
                    throw new NotImplementedException();
                }}
                "#
            );
        }
    }

    fn type_name(&mut self, ty: &Type) -> String {
        self.type_name_with_qualifier(ty, false)
    }

    // We use a global:: prefix to avoid conflicts with namespace clashes on partial namespace matches
    fn global_if_user_type(&self, ty: &Type) -> String {
        match ty {
            Type::Id(id) => {
                let ty = &self.resolve.types[*id];
                match &ty.kind {
                    TypeDefKind::Option(_ty) => "".to_owned(),
                    TypeDefKind::Result(_result) => "".to_owned(),
                    TypeDefKind::List(_list) => "".to_owned(),
                    TypeDefKind::Tuple(_tuple) => "".to_owned(),
                    TypeDefKind::Type(inner_type) => self.global_if_user_type(inner_type),
                    _ => "global::".to_owned(),
                }
            }
            _ => "".to_owned(),
        }
    }

    fn type_name_with_qualifier(&mut self, ty: &Type, qualifier: bool) -> String {
        match ty {
            Type::Bool => "bool".to_owned(),
            Type::U8 => "byte".to_owned(),
            Type::U16 => "ushort".to_owned(),
            Type::U32 => "uint".to_owned(),
            Type::U64 => "ulong".to_owned(),
            Type::S8 => "sbyte".to_owned(),
            Type::S16 => "short".to_owned(),
            Type::S32 => "int".to_owned(),
            Type::S64 => "long".to_owned(),
            Type::F32 => "float".to_owned(),
            Type::F64 => "double".to_owned(),
            Type::Char => "uint".to_owned(),
            Type::String => "string".to_owned(),
            Type::Id(id) => {
                let ty = &self.resolve.types[*id];
                match &ty.kind {
                    TypeDefKind::Type(ty) => self.type_name_with_qualifier(ty, qualifier),
                    TypeDefKind::List(ty) => {
                        if is_primitive(ty) {
                            format!("{}[]", self.type_name(ty))
                        } else {
                            format!("List<{}>", self.type_name_boxed(ty, qualifier))
                        }
                    }
                    TypeDefKind::Tuple(tuple) => {
                        let count = tuple.types.len();
                        self.gen.tuple_counts.insert(count);

                        let params = match count {
                            0 => String::new(),
                            1 => self.type_name_boxed(tuple.types.first().unwrap(), qualifier),
                            _ => format!(
                                "({})",
                                tuple
                                    .types
                                    .iter()
                                    .map(|ty| self.type_name_boxed(ty, qualifier))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        };

                        params
                    }
                    TypeDefKind::Option(base_ty) => {
                        self.gen.needs_option = true;
                        if let Some(_name) = &ty.name {
                            format!(
                                "Option<{}>",
                                self.type_name_with_qualifier(base_ty, qualifier)
                            )
                        } else {
                            format!(
                                "Option<{}>",
                                self.type_name_with_qualifier(base_ty, qualifier)
                            )
                        }
                    }
                    TypeDefKind::Result(result) => {
                        self.gen.needs_result = true;
                        let mut name = |ty: &Option<Type>| {
                            ty.as_ref()
                                .map(|ty| self.type_name_boxed(ty, qualifier))
                                .unwrap_or_else(|| "None".to_owned())
                        };
                        let ok = name(&result.ok);
                        let err = name(&result.err);

                        format!("Result<{ok}, {err}>")
                    }
                    TypeDefKind::Handle(handle) => {
                        let (Handle::Own(id) | Handle::Borrow(id)) = handle;
                        self.type_name_with_qualifier(&Type::Id(*id), qualifier)
                    }
                    _ => {
                        if let Some(name) = &ty.name {
                            format!(
                                "{}{}",
                                self.qualifier(qualifier, id),
                                name.to_upper_camel_case()
                            )
                        } else {
                            unreachable!("todo: {ty:?}")
                        }
                    }
                }
            }
        }
    }

    fn type_name_boxed(&mut self, ty: &Type, qualifier: bool) -> String {
        match ty {
            Type::Bool => "bool".into(),
            Type::U8 => "byte".into(),
            Type::U16 => "ushort".into(),
            Type::U32 => "uint".into(),
            Type::U64 => "ulong".into(),
            Type::S8 => "sbyte".into(),
            Type::S16 => "short".into(),
            Type::S32 => "int".into(),
            Type::S64 => "long".into(),
            Type::F32 => "float".into(),
            Type::F64 => "double".into(),
            Type::Char => "uint".into(),
            Type::Id(id) => {
                let def = &self.resolve.types[*id];
                match &def.kind {
                    TypeDefKind::Type(ty) => self.type_name_boxed(ty, qualifier),
                    _ => self.type_name_with_qualifier(ty, qualifier),
                }
            }
            _ => self.type_name_with_qualifier(ty, qualifier),
        }
    }

    fn print_docs(&mut self, docs: &Docs) {
        if let Some(docs) = &docs.contents {
            let lines = docs
                .trim()
                .lines()
                .map(|line| format!("* {line}"))
                .collect::<Vec<_>>()
                .join("\n");

            uwrite!(
                self.src,
                "
                /**
                 {lines}
                 */
                "
            )
        }
    }

    fn non_empty_type<'a>(&self, ty: Option<&'a Type>) -> Option<&'a Type> {
        if let Some(ty) = ty {
            let id = match ty {
                Type::Id(id) => *id,
                _ => return Some(ty),
            };
            match &self.resolve.types[id].kind {
                TypeDefKind::Type(t) => self.non_empty_type(Some(t)).map(|_| ty),
                TypeDefKind::Record(r) => (!r.fields.is_empty()).then_some(ty),
                TypeDefKind::Tuple(t) => (!t.types.is_empty()).then_some(ty),
                _ => Some(ty),
            }
        } else {
            None
        }
    }

    fn start_resource(
        &mut self,
        import_module_name: &str,
        id: TypeId,
        modifiers: &str,
        funcs: &[&Function],
    ) {
        let info = &self.gen.resources[&id];
        let name = info.name.clone();
        let upper_camel = name.to_upper_camel_case();
        let docs = info.docs.clone();
        self.print_docs(&docs);

        uwriteln!(
            self.src,
            r#"
            public {modifiers} class {upper_camel}: IDisposable {{
                internal int? handle;

                public void Dispose() {{
                    Dispose(true);
                    GC.SuppressFinalize(this);
                }}

                [DllImport("{import_module_name}", EntryPoint = "[resource-drop]{name}"), WasmImportLinkage]
                private static extern void wasmImportDrop(int p0);

                protected virtual void Dispose(bool disposing) {{
                    if (handle.HasValue) {{
                        wasmImportDrop((int) handle);
                        handle = null;
                    }}
                }}
            "#
        );

        if funcs
            .iter()
            .any(|f| matches!(&f.kind, FunctionKind::Constructor(_)))
            && !funcs
                .iter()
                .any(|f| matches!(&f.kind, FunctionKind::Constructor(_)) && f.params.is_empty())
        {
            uwriteln!(
                self.src,
                r#"
                internal {upper_camel}() {{ }}
                "#
            );
        }

        if self.gen.opts.generate_stub {
            let super_ = self.type_name_with_qualifier(&Type::Id(id), true);

            uwriteln!(
                self.stub,
                r#"
                public class {upper_camel}: {super_} {{
                "#
            );
        }
    }

    fn end_resource(&mut self) {
        if self.gen.opts.generate_stub {
            uwriteln!(
                self.stub,
                "
                }}
                "
            );
        }

        uwriteln!(
            self.src,
            "
            }}
            "
        );
    }

    fn sig_string(&mut self, func: &Function, qualifier: bool) -> String {
        let result_type = if let FunctionKind::Constructor(_) = &func.kind {
            String::new()
        } else {
            match func.results.len() {
                0 => "void".into(),
                1 => self
                    .type_name_with_qualifier(func.results.iter_types().next().unwrap(), qualifier),
                count => {
                    self.gen.tuple_counts.insert(count);
                    format!(
                        "({})",
                        func.results
                            .iter_types()
                            .map(|ty| self.type_name_boxed(ty, qualifier))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
        };

        let params = func
            .params
            .iter()
            .skip(if let FunctionKind::Method(_) = &func.kind {
                1
            } else {
                0
            })
            .map(|(name, ty)| {
                let ty = self.type_name_with_qualifier(ty, qualifier);
                let name = name.to_csharp_ident();
                format!("{ty} {name}")
            })
            .collect::<Vec<_>>()
            .join(", ");

        let (camel_name, modifiers) = match &func.kind {
            FunctionKind::Freestanding | FunctionKind::Static(_) => {
                (func.item_name().to_upper_camel_case(), "static ")
            }
            FunctionKind::Method(_) => (func.item_name().to_upper_camel_case(), "override "),
            FunctionKind::Constructor(id) => {
                (self.gen.resources[id].name.to_upper_camel_case(), "")
            }
        };

        format!("public {modifiers} {result_type} {camel_name}({params})")
    }
}

impl<'a> wit_bindgen_core::InterfaceGenerator<'a> for InterfaceGenerator<'a> {
    fn resolve(&self) -> &'a Resolve {
        self.resolve
    }

    fn type_record(&mut self, _id: TypeId, name: &str, record: &Record, docs: &Docs) {
        self.print_docs(docs);

        let name = name.to_upper_camel_case();

        let parameters = record
            .fields
            .iter()
            .map(|field| {
                format!(
                    "{} {}",
                    self.type_name(&field.ty),
                    field.name.to_csharp_ident()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");

        let assignments = record
            .fields
            .iter()
            .map(|field| {
                let name = field.name.to_csharp_ident();
                format!("this.{name} = {name};")
            })
            .collect::<Vec<_>>()
            .join("\n");

        let fields = if record.fields.is_empty() {
            format!("public const {name} INSTANCE = new {name}();")
        } else {
            record
                .fields
                .iter()
                .map(|field| {
                    format!(
                        "public readonly {} {};",
                        self.type_name(&field.ty),
                        field.name.to_csharp_ident()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        uwrite!(
            self.src,
            "
            public class {name} {{
                {fields}

                public {name}({parameters}) {{
                    {assignments}
                }}
            }}
            "
        );
    }

    fn type_flags(&mut self, _id: TypeId, name: &str, flags: &Flags, docs: &Docs) {
        self.print_docs(docs);

        let name = name.to_upper_camel_case();

        let enum_elements = flags
            .flags
            .iter()
            .enumerate()
            .map(|(i, flag)| {
                let flag_name = flag.name.to_shouty_snake_case();
                let suffix = if matches!(flags.repr(), FlagsRepr::U32(2)) {
                    "UL"
                } else {
                    ""
                };
                format!("{flag_name} = 1{suffix} << {i},")
            })
            .collect::<Vec<_>>()
            .join("\n");

        let enum_type = match flags.repr() {
            FlagsRepr::U32(2) => ": ulong",
            FlagsRepr::U16 => ": ushort",
            FlagsRepr::U8 => ": byte",
            _ => "",
        };

        uwrite!(
            self.src,
            "
            public enum {name} {enum_type} {{
                {enum_elements}
            }}
            "
        );
    }

    fn type_tuple(&mut self, id: TypeId, _name: &str, _tuple: &Tuple, _docs: &Docs) {
        self.type_name(&Type::Id(id));
    }

    fn type_variant(&mut self, _id: TypeId, name: &str, variant: &Variant, docs: &Docs) {
        self.print_docs(docs);

        let name = name.to_upper_camel_case();
        let tag_type = int_type(variant.tag());

        let constructors = variant
            .cases
            .iter()
            .map(|case| {
                let case_name = case.name.to_csharp_ident();
                let tag = case.name.to_shouty_snake_case();
                let (parameter, argument) = if let Some(ty) = self.non_empty_type(case.ty.as_ref())
                {
                    (
                        format!("{} {case_name}", self.type_name(ty)),
                        case_name.deref(),
                    )
                } else {
                    (String::new(), "null")
                };

                format!(
                    "public static {name} {case_name}({parameter}) {{
                         return new {name}({tag}, {argument});
                     }}
                    "
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let accessors = variant
            .cases
            .iter()
            .filter_map(|case| {
                self.non_empty_type(case.ty.as_ref()).map(|ty| {
                    let case_name = case.name.to_upper_camel_case();
                    let tag = case.name.to_shouty_snake_case();
                    let ty = self.type_name(ty);
                    format!(
                        r#"public {ty} As{case_name} 
                        {{ 
                            get 
                            {{
                                if (Tag == {tag}) 
                                    return ({ty})value;
                                else 
                                    throw new ArgumentException("expected {tag}, got " + Tag);
                            }} 
                        }}
                        "#
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("\n");

        let tags = variant
            .cases
            .iter()
            .enumerate()
            .map(|(i, case)| {
                let tag = case.name.to_shouty_snake_case();
                format!("public const {tag_type} {tag} = {i};")
            })
            .collect::<Vec<_>>()
            .join("\n");

        uwrite!(
            self.src,
            "
            public class {name} {{
                public readonly {tag_type} Tag;
                private readonly object value;

                private {name}({tag_type} tag, object value) {{
                    this.Tag = tag;
                    this.value = value;
                }}

                {constructors}
                {accessors}
                {tags}
            }}
            "
        );
    }

    fn type_option(&mut self, id: TypeId, _name: &str, _payload: &Type, _docs: &Docs) {
        self.type_name(&Type::Id(id));
    }

    fn type_result(&mut self, id: TypeId, _name: &str, _result: &Result_, _docs: &Docs) {
        self.type_name(&Type::Id(id));
    }

    fn type_enum(&mut self, _id: TypeId, name: &str, enum_: &Enum, docs: &Docs) {
        self.print_docs(docs);

        let name = name.to_upper_camel_case();

        let cases = enum_
            .cases
            .iter()
            .map(|case| case.name.to_shouty_snake_case())
            .collect::<Vec<_>>()
            .join(", ");

        uwrite!(
            self.src,
            "
            public enum {name} {{
                {cases}
            }}
            "
        );
    }

    fn type_alias(&mut self, id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {
        self.type_name(&Type::Id(id));
    }

    fn type_list(&mut self, id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {
        self.type_name(&Type::Id(id));
    }

    fn type_builtin(&mut self, _id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {
        unimplemented!();
    }

    fn type_resource(&mut self, id: TypeId, name: &str, docs: &Docs) {
        self.gen
            .resources
            .entry(id)
            .or_insert_with(|| ResourceInfo {
                name: name.to_owned(),
                docs: docs.clone(),
                direction: Direction::Import,
            })
            .direction = self.direction;
    }
}

enum Stubs<'a> {
    World(&'a Vec<InterfaceFragment>),
    Interface(&'a Vec<InterfaceFragment>),
}

struct Block {
    body: String,
    results: Vec<String>,
    element: String,
    base: String,
}

struct Cleanup {
    address: String,
}

struct BlockStorage {
    body: String,
    element: String,
    base: String,
    cleanup: Vec<Cleanup>,
}

struct FunctionBindgen<'a, 'b> {
    gen: &'b mut InterfaceGenerator<'a>,
    func_name: &'b str,
    kind: &'b FunctionKind,
    params: Box<[String]>,
    src: String,
    locals: Ns,
    block_storage: Vec<BlockStorage>,
    blocks: Vec<Block>,
    payloads: Vec<String>,
    needs_cleanup_list: bool,
    cleanup: Vec<Cleanup>,
    import_return_pointer_area_size: usize,
    import_return_pointer_area_align: usize,
    resource_drops: Vec<String>,
}

impl<'a, 'b> FunctionBindgen<'a, 'b> {
    fn new(
        gen: &'b mut InterfaceGenerator<'a>,
        func_name: &'b str,
        kind: &'b FunctionKind,
        params: Box<[String]>,
    ) -> FunctionBindgen<'a, 'b> {
        Self {
            gen,
            func_name,
            kind,
            params,
            src: String::new(),
            locals: Ns::default(),
            block_storage: Vec::new(),
            blocks: Vec::new(),
            payloads: Vec::new(),
            needs_cleanup_list: false,
            cleanup: Vec::new(),
            import_return_pointer_area_size: 0,
            import_return_pointer_area_align: 0,
            resource_drops: Vec::new(),
        }
    }

    fn lower_variant(
        &mut self,
        cases: &[(&str, Option<Type>)],
        lowered_types: &[WasmType],
        op: &str,
        results: &mut Vec<String>,
    ) {
        let blocks = self
            .blocks
            .drain(self.blocks.len() - cases.len()..)
            .collect::<Vec<_>>();

        let payloads = self
            .payloads
            .drain(self.payloads.len() - cases.len()..)
            .collect::<Vec<_>>();

        let lowered = lowered_types
            .iter()
            .map(|_| self.locals.tmp("lowered"))
            .collect::<Vec<_>>();

        results.extend(lowered.iter().cloned());

        let declarations = lowered
            .iter()
            .zip(lowered_types)
            .map(|(lowered, ty)| format!("{} {lowered};", wasm_type(*ty)))
            .collect::<Vec<_>>()
            .join("\n");

        let cases = cases
            .iter()
            .zip(blocks)
            .zip(payloads)
            .enumerate()
            .map(
                |(i, (((name, ty), Block { body, results, .. }), payload))| {
                    let payload = if let Some(ty) = self.gen.non_empty_type(ty.as_ref()) {
                        let ty = self.gen.type_name_with_qualifier(ty, true);
                        let name = name.to_upper_camel_case();

                        format!("{ty} {payload} = {op}.As{name};")
                    } else {
                        String::new()
                    };

                    let assignments = lowered
                        .iter()
                        .zip(&results)
                        .map(|(lowered, result)| format!("{lowered} = {result};\n"))
                        .collect::<Vec<_>>()
                        .concat();

                    format!(
                        "case {i}: {{
                         {payload}
                         {body}
                         {assignments}
                         break;
                     }}"
                    )
                },
            )
            .collect::<Vec<_>>()
            .join("\n");

        uwrite!(
            self.src,
            r#"
            {declarations}

            switch ({op}.Tag) {{
                {cases}

                default: throw new ArgumentException($"invalid discriminant: {{{op}}}");
            }}
            "#
        );
    }

    fn lift_variant(
        &mut self,
        ty: &Type,
        cases: &[(&str, Option<Type>)],
        op: &str,
        results: &mut Vec<String>,
    ) {
        let blocks = self
            .blocks
            .drain(self.blocks.len() - cases.len()..)
            .collect::<Vec<_>>();
        let ty = self.gen.type_name_with_qualifier(ty, true);
        //let ty = self.gen.type_name(ty);
        let generics_position = ty.find('<');
        let lifted = self.locals.tmp("lifted");

        let cases = cases
            .iter()
            .zip(blocks)
            .enumerate()
            .map(|(i, ((case_name, case_ty), Block { body, results, .. }))| {
                let payload = if self.gen.non_empty_type(case_ty.as_ref()).is_some() {
                    results.into_iter().next().unwrap()
                } else if generics_position.is_some() {
                    if let Some(ty) = case_ty.as_ref() {
                        format!("{}.INSTANCE", self.gen.type_name_with_qualifier(ty, true))
                    } else {
                        format!("new {}None()", self.gen.gen.qualifier())
                    }
                } else {
                    String::new()
                };

                let method = case_name.to_csharp_ident();

                let call = if let Some(position) = generics_position {
                    let (ty, generics) = ty.split_at(position);
                    format!("{ty}{generics}.{method}")
                } else {
                    format!("{ty}.{method}")
                };

                format!(
                    "case {i}: {{
                         {body}
                         {lifted} = {call}({payload});
                         break;
                     }}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        uwrite!(
            self.src,
            r#"
            {ty} {lifted};

            switch ({op}) {{
                {cases}

                default: throw new ArgumentException($"invalid discriminant: {{{op}}}");
            }}
            "#
        );

        results.push(lifted);
    }
}

impl Bindgen for FunctionBindgen<'_, '_> {
    type Operand = String;

    fn emit(
        &mut self,
        _resolve: &Resolve,
        inst: &Instruction<'_>,
        operands: &mut Vec<String>,
        results: &mut Vec<String>,
    ) {
        match inst {
            Instruction::GetArg { nth } => results.push(self.params[*nth].clone()),
            Instruction::I32Const { val } => results.push(val.to_string()),
            Instruction::ConstZero { tys } => results.extend(tys.iter().map(|ty| {
                match ty {
                    WasmType::I32 => "0",
                    WasmType::I64 => "0L",
                    WasmType::F32 => "0.0F",
                    WasmType::F64 => "0.0D",
                    WasmType::Pointer => "0",
                    WasmType::PointerOrI64 => "0L",
                    WasmType::Length => "0",
                }
                .to_owned()
            })),
            Instruction::I32Load { offset }
            | Instruction::PointerLoad { offset }
            | Instruction::LengthLoad { offset } => results.push(format!("BitConverter.ToInt32(new Span<byte>((void*)({} + {offset}), 4))",operands[0])),
            Instruction::I32Load8U { offset } => results.push(format!("new Span<byte>((void*)({} + {offset}), 1)[0]",operands[0])),
            Instruction::I32Load8S { offset } => results.push(format!("(sbyte)new Span<byte>((void*)({} + {offset}), 1)[0]",operands[0])),
            Instruction::I32Load16U { offset } => results.push(format!("BitConverter.ToUInt16(new Span<byte>((void*)({} + {offset}), 2))",operands[0])),
            Instruction::I32Load16S { offset } => results.push(format!("BitConverter.ToInt16(new Span<byte>((void*)({} + {offset}), 2))",operands[0])),
            Instruction::I64Load { offset } => results.push(format!("BitConverter.ToInt64(new Span<byte>((void*)({} + {offset}), 8))",operands[0])),
            Instruction::F32Load { offset } => results.push(format!("BitConverter.ToSingle(new Span<byte>((void*)({} + {offset}), 4))",operands[0])),
            Instruction::F64Load { offset } => results.push(format!("BitConverter.ToDouble(new Span<byte>((void*)({} + {offset}), 8))",operands[0])),
            Instruction::I32Store { offset }
            | Instruction::PointerStore { offset }
            | Instruction::LengthStore { offset } => uwriteln!(self.src, "BitConverter.TryWriteBytes(new Span<byte>((void*)({} + {offset}), 4), unchecked((int){}));", operands[1], operands[0]),
            Instruction::I32Store8 { offset } => uwriteln!(self.src, "*(byte*)({} + {offset}) = (byte){};", operands[1], operands[0]),
            Instruction::I32Store16 { offset } => uwriteln!(self.src, "BitConverter.TryWriteBytes(new Span<byte>((void*)({} + {offset}), 2), (short){});", operands[1], operands[0]),
            Instruction::I64Store { offset } => uwriteln!(self.src, "BitConverter.TryWriteBytes(new Span<byte>((void*)({} + {offset}), 8), unchecked((long){}));", operands[1], operands[0]),
            Instruction::F32Store { offset } => uwriteln!(self.src, "BitConverter.TryWriteBytes(new Span<byte>((void*)({} + {offset}), 4), unchecked((float){}));", operands[1], operands[0]),
            Instruction::F64Store { offset } => uwriteln!(self.src, "BitConverter.TryWriteBytes(new Span<byte>((void*)({} + {offset}), 8), unchecked((double){}));", operands[1], operands[0]),

            Instruction::I64FromU64 => results.push(format!("unchecked((long)({}))", operands[0])),
            Instruction::I32FromChar => results.push(format!("((int){})", operands[0])),
            Instruction::I32FromU32 => results.push(format!("unchecked((int)({}))", operands[0])),
            Instruction::U8FromI32 => results.push(format!("((byte){})", operands[0])),
            Instruction::S8FromI32 => results.push(format!("((sbyte){})", operands[0])),
            Instruction::U16FromI32 => results.push(format!("((ushort){})", operands[0])),
            Instruction::S16FromI32 => results.push(format!("((short){})", operands[0])),
            Instruction::U32FromI32 => results.push(format!("unchecked((uint)({}))", operands[0])),
            Instruction::U64FromI64 => results.push(format!("unchecked((ulong)({}))", operands[0])),
            Instruction::CharFromI32 => results.push(format!("unchecked((uint)({}))", operands[0])),

            Instruction::I64FromS64
            | Instruction::I32FromU16
            | Instruction::I32FromS16
            | Instruction::I32FromU8
            | Instruction::I32FromS8
            | Instruction::I32FromS32
            | Instruction::F32FromCoreF32
            | Instruction::CoreF32FromF32
            | Instruction::CoreF64FromF64
            | Instruction::F64FromCoreF64
            | Instruction::S32FromI32
            | Instruction::S64FromI64 => results.push(operands[0].clone()),

            Instruction::Bitcasts { casts } => {
                results.extend(casts.iter().zip(operands).map(|(cast, op)| perform_cast(op, cast)))
            }

            Instruction::I32FromBool => {
                results.push(format!("({} ? 1 : 0)", operands[0]));
            }
            Instruction::BoolFromI32 => results.push(format!("({} != 0)", operands[0])),

            Instruction::FlagsLower {
                flags,
                name: _,
                ty: _,
            } => {
                if flags.flags.len() > 32 {
                    results.push(format!(
                        "unchecked((int)(((long){}) & uint.MaxValue))",
                        operands[0].to_string()
                    ));
                    results.push(format!(
                        "unchecked(((int)((long){} >> 32)))",
                        operands[0].to_string()
                    ));
                } else {
                    results.push(format!("(int){}", operands[0].to_string()));
                }
            }

            Instruction::FlagsLift { flags, name, ty } => {
                let qualified_type_name = format!(
                    "{}{}",
                    self.gen.qualifier(true, ty),
                    name.to_string().to_upper_camel_case()
                );
                if flags.flags.len() > 32 {
                    results.push(format!(
                        "({})(unchecked((uint)({})) | (ulong)(unchecked((uint)({}))) << 32)",
                        qualified_type_name,
                        operands[0].to_string(),
                        operands[1].to_string()
                    ));
                } else {
                    results.push(format!("({})({})", qualified_type_name, operands[0]))
                }
            }

            Instruction::RecordLower { record, .. } => {
                let op = &operands[0];
                for f in record.fields.iter() {
                    results.push(format!("({}).{}", op, f.name.to_csharp_ident()));
                }
            }
            Instruction::RecordLift { ty, name, .. } => {
                let qualified_type_name = format!(
                    "{}{}",
                    self.gen.qualifier(true, ty),
                    name.to_string().to_upper_camel_case()
                );
                let mut result = format!("new {} (\n", qualified_type_name);

                result.push_str(&operands.join(","));
                result.push_str(")");

                results.push(result);
            }
            Instruction::TupleLift { .. } => {
                let mut result = String::from("(");

                uwriteln!(result, "{}", operands.join(","));

                result.push_str(")");
                results.push(result);
            }

            Instruction::TupleLower { tuple, ty: _ } => {
                let op = &operands[0];
                match tuple.types.len() {
                    1 => results.push(format!("({})", op)),
                    _ => {
                        for i in 0..tuple.types.len() {
                            results.push(format!("({}).Item{}", op, i + 1));
                        }
                    }
                }
            }

            Instruction::VariantPayloadName => {
                let payload = self.locals.tmp("payload");
                results.push(payload.clone());
                self.payloads.push(payload);
            }

            Instruction::VariantLower {
                variant,
                results: lowered_types,
                ..
            } => self.lower_variant(
                &variant
                    .cases
                    .iter()
                    .map(|case| (case.name.deref(), case.ty))
                    .collect::<Vec<_>>(),
                lowered_types,
                &operands[0],
                results,
            ),

            Instruction::VariantLift { variant, ty, .. } => self.lift_variant(
                &Type::Id(*ty),
                &variant
                    .cases
                    .iter()
                    .map(|case| (case.name.deref(), case.ty))
                    .collect::<Vec<_>>(),
                &operands[0],
                results,
            ),

            Instruction::OptionLower {
                results: lowered_types,
                payload,
                ..
            } => {
                let some = self.blocks.pop().unwrap();
                let none = self.blocks.pop().unwrap();
                let some_payload = self.payloads.pop().unwrap();
                let none_payload = self.payloads.pop().unwrap();

                let lowered = lowered_types
                    .iter()
                    .map(|_| self.locals.tmp("lowered"))
                    .collect::<Vec<_>>();

                results.extend(lowered.iter().cloned());

                let declarations = lowered
                    .iter()
                    .zip(lowered_types.iter())
                    .map(|(lowered, ty)| format!("{} {lowered};", wasm_type(*ty)))
                    .collect::<Vec<_>>()
                    .join("\n");

                let op = &operands[0];

                let block = |ty: Option<&Type>, Block { body, results, .. }, payload| {
                    let payload = if let Some(_ty) = self.gen.non_empty_type(ty) {
                        format!("var {payload} = ({op}).Value;")
                    } else {
                        String::new()
                    };

                    let assignments = lowered
                        .iter()
                        .zip(&results)
                        .map(|(lowered, result)| format!("{lowered} = {result};\n"))
                        .collect::<Vec<_>>()
                        .concat();

                    format!(
                        "{payload}
                         {body}
                         {assignments}"
                    )
                };

                let none = block(None, none, none_payload);
                let some = block(Some(payload), some, some_payload);

                uwrite!(
                    self.src,
                    r#"
                    {declarations}

                    if (({op}).HasValue) {{
                        {some}
                    }} else {{
                        {none}
                    }}
                    "#
                );
            }

            Instruction::OptionLift { payload, ty } => {
                let some = self.blocks.pop().unwrap();
                let _none = self.blocks.pop().unwrap();

                let ty = self.gen.type_name_with_qualifier(&Type::Id(*ty), true);
                let lifted = self.locals.tmp("lifted");
                let op = &operands[0];

                let payload = if self.gen.non_empty_type(Some(*payload)).is_some() {
                    some.results.into_iter().next().unwrap()
                } else {
                    "null".into()
                };

                let some = some.body;

                uwrite!(
                    self.src,
                    r#"
                    {ty} {lifted};

                    switch ({op}) {{
                        case 0: {{
                            {lifted} = {ty}.None;
                            break;
                        }}

                        case 1: {{
                            {some}
                            {lifted} = new ({payload});
                            break;
                        }}

                        default: throw new ArgumentException("invalid discriminant: " + ({op}));
                    }}
                    "#
                );

                results.push(lifted);
            }

            Instruction::ResultLower {
                results: lowered_types,
                result,
                ..
            } => self.lower_variant(
                &[("ok", result.ok), ("err", result.err)],
                lowered_types,
                &operands[0],
                results,
            ),

            Instruction::ResultLift { result, ty } => self.lift_variant(
                &Type::Id(*ty),
                &[("ok", result.ok), ("err", result.err)],
                &operands[0],
                results,
            ),

            Instruction::EnumLower { .. } => results.push(format!("(int){}", operands[0])),

            Instruction::EnumLift { ty, .. } => {
                let t = self.gen.type_name_with_qualifier(&Type::Id(*ty), true);
                let op = &operands[0];
                results.push(format!("({}){}", t, op));

                // uwriteln!(
                //    self.src,
                //    "Debug.Assert(Enum.IsDefined(typeof({}), {}));",
                //    t,
                //    op
                // );
            }

            Instruction::ListCanonLower { element, realloc } => {
                let list = &operands[0];
                let (_size, ty) = list_element_info(element);

                match self.gen.direction {
                    Direction::Import => {
                        let buffer: String = self.locals.tmp("buffer");
                        uwrite!(
                            self.src,
                            "
                            void* {buffer} = stackalloc {ty}[({list}).Length];
                            {list}.AsSpan<{ty}>().CopyTo(new Span<{ty}>({buffer}, {list}.Length));
                            "
                        );
                        results.push(format!("(int){buffer}"));
                        results.push(format!("({list}).Length"));
                    }
                    Direction::Export => {
                        let address = self.locals.tmp("address");
                        let buffer = self.locals.tmp("buffer");
                        let gc_handle = self.locals.tmp("gcHandle");
                        let size = self.gen.gen.sizes.size(element);
                        uwrite!(
                            self.src,
                            "
                        byte[] {buffer} = new byte[({size}) * {list}.Count()];
                        Buffer.BlockCopy({list}.ToArray(), 0, {buffer}, 0, ({size}) * {list}.Count());
                        var {gc_handle} = GCHandle.Alloc({buffer}, GCHandleType.Pinned);
                        var {address} = {gc_handle}.AddrOfPinnedObject();
                        "
                        );

                        if realloc.is_none() {
                            self.cleanup.push(Cleanup {
                                address: gc_handle.clone(),
                            });
                        }
                        results.push(format!("((IntPtr)({address})).ToInt32()"));
                        results.push(format!("{list}.Count()"));
                    }
                }
            }

            Instruction::ListCanonLift { element, .. } => {
                let (_, ty) = list_element_info(element);
                let array = self.locals.tmp("array");
                let address = &operands[0];
                let length = &operands[1];

                uwrite!(
                    self.src,
                    "
                    var {array} = new {ty}[{length}];         
                    new Span<{ty}>((void*)({address}), {length}).CopyTo(new Span<{ty}>({array}));          
                    "
                );

                results.push(array);
            }

            Instruction::StringLower { realloc } => {
                let op = &operands[0];
                let interop_string = self.locals.tmp("interopString");
                let result_var = self.locals.tmp("result");
                uwriteln!(
                    self.src,
                    "
                    var {result_var} = {op};
                    IntPtr {interop_string} = InteropString.FromString({result_var}, out int length{result_var});"
                );

                if realloc.is_none() {
                    results.push(format!("{interop_string}.ToInt32()"));
                } else {
                    results.push(format!("{interop_string}.ToInt32()"));
                }
                results.push(format!("length{result_var}"));

                self.gen.gen.needs_interop_string = true;
            }

            Instruction::StringLift { .. } => results.push(format!(
                "Encoding.UTF8.GetString((byte*){}, {})",
                operands[0], operands[1]
            )),

            Instruction::ListLower { element, realloc } => {
                let Block {
                    body,
                    results: block_results,
                    element: block_element,
                    base,
                } = self.blocks.pop().unwrap();
                assert!(block_results.is_empty());

                let list = &operands[0];
                let size = self.gen.gen.sizes.size(element);
                let _align = self.gen.gen.sizes.align(element);
                let ty = self.gen.type_name(element);
                let index = self.locals.tmp("index");

                let buffer: String = self.locals.tmp("buffer");
                let gc_handle = self.locals.tmp("gcHandle");
                let address = self.locals.tmp("address");

                uwrite!(
                    self.src,
                    "
                    byte[] {buffer} = new byte[{size} * {list}.Count()];
                    var {gc_handle} = GCHandle.Alloc({buffer}, GCHandleType.Pinned);
                    var {address} = {gc_handle}.AddrOfPinnedObject();

                    for (int {index} = 0; {index} < {list}.Count(); ++{index}) {{
                        {ty} {block_element} = {list}[{index}];
                        int {base} = (int){address} + ({index} * {size});
                        {body}
                    }}
                    "
                );

                if realloc.is_none() {
                    self.cleanup.push(Cleanup {
                        address: gc_handle.clone(),
                    });
                }

                results.push(format!("(int){address}"));
                results.push(format!("{list}.Count()"));
            }

            Instruction::ListLift { element, .. } => {
                let Block {
                    body,
                    results: block_results,
                    base,
                    ..
                } = self.blocks.pop().unwrap();
                let address = &operands[0];
                let length = &operands[1];
                let array = self.locals.tmp("array");
                let ty = self.gen.type_name(element);
                let size = self.gen.gen.sizes.size(element);
                let _align = self.gen.gen.sizes.align(element);
                let index = self.locals.tmp("index");

                let result = match &block_results[..] {
                    [result] => result,
                    _ => todo!("result count == {}", results.len()),
                };

                uwrite!(
                    self.src,
                    "
                    var {array} = new List<{ty}>({length});
                    for (int {index} = 0; {index} < {length}; ++{index}) {{
                        int {base} = {address} + ({index} * {size});
                        {body}
                        {array}.Add({result});
                    }}
                    "
                );

                results.push(array);
            }

            Instruction::IterElem { .. } => {
                results.push(self.block_storage.last().unwrap().element.clone())
            }

            Instruction::IterBasePointer => {
                results.push(self.block_storage.last().unwrap().base.clone())
            }

            Instruction::CallWasm { sig, .. } => {
                let assignment = match &sig.results[..] {
                    [_] => {
                        let result = self.locals.tmp("result");
                        let assignment = format!("var {result} = ");
                        results.push(result);
                        assignment
                    }

                    [] => String::new(),

                    _ => unreachable!(),
                };

                let func_name = self.func_name.to_upper_camel_case();

                let operands = operands.join(", ");

                uwriteln!(
                    self.src,
                    "{assignment} {func_name}WasmInterop.wasmImport{func_name}({operands});"
                );
            }

            Instruction::CallInterface { func } => {
                let module = self.gen.name.to_string();
                let func_name = self.func_name.to_upper_camel_case();
                let interface_name = CSharp::get_class_name_from_qualified_name(module).1;

                let class_name_root = (match self.gen.function_level {
                    FunctionLevel::Interface => interface_name
                        .strip_prefix("I")
                        .unwrap()
                        .to_upper_camel_case(),
                    FunctionLevel::FreeStanding => interface_name,
                })
                .to_upper_camel_case();

                let mut oper = String::new();

                for (i, param) in operands.iter().enumerate() {
                    if i == 0 && matches!(self.kind, FunctionKind::Method(_)) {
                        continue;
                    }

                    oper.push_str(&format!("({param})"));

                    if i < operands.len() && operands.len() != i + 1 {
                        oper.push_str(", ");
                    }
                }

                match self.kind {
                    FunctionKind::Freestanding | FunctionKind::Static(_) | FunctionKind::Method(_) => {
                        let target = match self.kind {
                            FunctionKind::Static(id) => format!(
                                "{class_name_root}Impl.{}",
                                self.gen.type_name_with_qualifier(&Type::Id(*id), false)
                            ),
                            FunctionKind::Method(_) => operands[0].clone(),
                            _ => format!("{class_name_root}Impl")
                        };

                        match func.results.len() {
                            0 => uwriteln!(self.src, "{target}.{func_name}({oper});"),
                            1 => {
                                let ret = self.locals.tmp("ret");
                                uwriteln!(
                                    self.src,
                                    "var {ret} = {target}.{func_name}({oper});"
                                );
                                results.push(ret);
                            }
                            _ => {
                                let ret = self.locals.tmp("ret");
                                uwriteln!(
                                    self.src,
                                    "var {ret} = {target}.{func_name}({oper});"
                                );
                                let mut i = 1;
                                for _ in func.results.iter_types() {
                                    results.push(format!("{ret}.Item{i}"));
                                    i += 1;
                                }
                            }
                        }
                    }
                    FunctionKind::Constructor(id) => {
                        let target = format!(
                            "{class_name_root}Impl.{}",
                            self.gen.type_name_with_qualifier(&Type::Id(*id), false)
                        );
                        let ret = self.locals.tmp("ret");
                        uwriteln!(self.src, "var {ret} = new {target}({oper});");
                        results.push(ret);
                    }
                }

                for drop in mem::take(&mut self.resource_drops) {
                    uwriteln!(self.src, "{drop}");
                }
            }

            Instruction::Return { amt: _, func } => {
                for Cleanup { address } in &self.cleanup {
                    uwriteln!(self.src, "{address}.Free();");
                }

                match self.kind {
                    FunctionKind::Constructor(_) => (),
                    _ => match func.results.len() {
                        0 => (),
                        1 => uwriteln!(self.src, "return {};", operands[0]),
                        _ => {
                            let results = operands.join(", ");
                            uwriteln!(self.src, "return ({results});")
                        }
                    }
                }
            }

            Instruction::Malloc { .. } => unimplemented!(),

            Instruction::GuestDeallocate { .. } => todo!("GuestDeallocate"),

            Instruction::GuestDeallocateString => todo!("GuestDeallocateString"),

            Instruction::GuestDeallocateVariant { .. } => todo!("GuestDeallocateString"),

            Instruction::GuestDeallocateList { .. } => todo!("GuestDeallocateList"),

            Instruction::HandleLower {
                handle,
                ..
            } => {
                let (Handle::Own(ty) | Handle::Borrow(ty)) = handle;
                let is_own = matches!(handle, Handle::Own(_));
                let handle = self.locals.tmp("handle");
                let ResourceInfo { direction, .. } = &self.gen.gen.resources[&dealias(self.gen.resolve, *ty)];
                let op = &operands[0];

                uwriteln!(self.src, "var {handle} = {op}.handle;");
                if let Direction::Import = direction {
                    if is_own {
                        uwriteln!(self.src, "{op}.handle = null;");
                    }
                } else {
                    self.gen.gen.needs_rep_table = true;
                    let local_rep = self.locals.tmp("localRep");
                    if is_own {
                        uwriteln!(
                            self.src,
                            "if (!handle.HasValue) {{
                                 var {local_rep} = RepTable.Add({op});
                                 {handle} = wasmImportResourceNew({local_rep});
                                 {op}.handle = {handle};
                             }}"
                        );
                    } else {
                        uwriteln!(
                            self.src,
                            "if (!handle.HasValue) {{
                                 var {local_rep} = RepTable.Add({op});
                                 {op}.handle = {local_rep};
                             }}"
                        );
                    }
                }
                results.push(format!("((int) handle)"));
            }

            Instruction::HandleLift {
                handle,
                ..
            } => {
                let (Handle::Own(ty) | Handle::Borrow(ty)) = handle;
                let is_own = matches!(handle, Handle::Own(_));
                let mut resource = self.locals.tmp("resource");
                let id = dealias(self.gen.resolve, *ty);
                let upper_camel = self.gen.type_name_with_qualifier(&Type::Id(id), true);
                let ResourceInfo { direction, .. } = &self.gen.gen.resources[&id];
                let op = &operands[0];

                if let Direction::Import = direction {
                    if let FunctionKind::Constructor(_) = self.kind {
                        resource = "this".to_owned();
                        uwriteln!(self.src,"{resource}.handle = {op};");
                    } else {
                        uwriteln!(
                            self.src,
                            "var {resource} = new {upper_camel}();
                             {resource}.handle = {op};"
                        );
                    }
                    if !is_own {
                        self.resource_drops.push(format!("{resource}.Dispose();"));
                    }
                } else {
                    self.gen.gen.needs_rep_table = true;
                    if is_own {
                        uwriteln!(
                            self.src,
                            "var {resource} = ({upper_camel}) RepTable.Remove(wasmImportResourceRep({op}));
                             {resource}.handle = null;"
                        );
                    } else {
                        uwriteln!(self.src, "var {resource} = ({upper_camel}) RepTable.Get({op});");
                    }
                }
                results.push(resource);
            }
        }
    }

    fn return_pointer(&mut self, size: usize, align: usize) -> String {
        let ptr = self.locals.tmp("ptr");

        // Use a stack-based return area for imports, because exports need
        // their return area to be live until the post-return call.
        match self.gen.direction {
            Direction::Import => {
                let ret_area = self.locals.tmp("retArea");
                let name = self.func_name.to_upper_camel_case();
                self.import_return_pointer_area_size =
                    self.import_return_pointer_area_size.max(size);
                self.import_return_pointer_area_align =
                    self.import_return_pointer_area_align.max(align);
                uwrite!(
                    self.src,
                    "
                    var {ret_area} = new {name}WasmInterop.ImportReturnArea();
                    var {ptr} = {ret_area}.AddressOfReturnArea();
                    "
                );

                return format!("{ptr}");
            }
            Direction::Export => {
                self.gen.gen.return_area_size = self.gen.gen.return_area_size.max(size);
                self.gen.gen.return_area_align = self.gen.gen.return_area_align.max(align);

                uwrite!(
                    self.src,
                    "
                    var {ptr} = InteropReturnArea.returnArea.AddressOfReturnArea();
                    "
                );
                self.gen.gen.needs_export_return_area = true;

                return format!("{ptr}");
            }
        }
    }

    fn push_block(&mut self) {
        self.block_storage.push(BlockStorage {
            body: mem::take(&mut self.src),
            element: self.locals.tmp("element"),
            base: self.locals.tmp("basePtr"),
            cleanup: mem::take(&mut self.cleanup),
        });
    }

    fn finish_block(&mut self, operands: &mut Vec<String>) {
        let BlockStorage {
            body,
            element,
            base,
            cleanup,
        } = self.block_storage.pop().unwrap();

        if !self.cleanup.is_empty() {
            //self.needs_cleanup_list = true;

            for Cleanup { address } in &self.cleanup {
                uwriteln!(self.src, "{address}.Free();");
            }
        }

        self.cleanup = cleanup;

        self.blocks.push(Block {
            body: mem::replace(&mut self.src, body),
            results: mem::take(operands),
            element: element,
            base: base,
        });
    }

    fn sizes(&self) -> &SizeAlign {
        &self.gen.gen.sizes
    }

    fn is_list_canonical(&self, _resolve: &Resolve, element: &Type) -> bool {
        is_primitive(element)
    }
}

fn perform_cast(op: &String, cast: &Bitcast) -> String {
    match cast {
        Bitcast::I32ToF32 => format!("BitConverter.Int32BitsToSingle({op})"),
        Bitcast::I64ToF32 => format!("BitConverter.Int32BitsToSingle((int){op})"),
        Bitcast::F32ToI32 => format!("BitConverter.SingleToInt32Bits({op})"),
        Bitcast::F32ToI64 => format!("BitConverter.SingleToInt32Bits({op})"),
        Bitcast::I64ToF64 => format!("BitConverter.Int64BitsToDouble({op})"),
        Bitcast::F64ToI64 => format!("BitConverter.DoubleToInt64Bits({op})"),
        Bitcast::I32ToI64 => format!("(long) ({op})"),
        Bitcast::I64ToI32 => format!("(int) ({op})"),
        Bitcast::I64ToP64 => format!("{op}"),
        Bitcast::P64ToI64 => format!("{op}"),
        Bitcast::LToI64 | Bitcast::PToP64 => format!("(long) ({op})"),
        Bitcast::I64ToL | Bitcast::P64ToP => format!("(int) ({op})"),
        Bitcast::I32ToP
        | Bitcast::PToI32
        | Bitcast::I32ToL
        | Bitcast::LToI32
        | Bitcast::LToP
        | Bitcast::PToL
        | Bitcast::None => op.to_owned(),
        Bitcast::Sequence(sequence) => {
            let [first, second] = &**sequence;
            perform_cast(&perform_cast(op, first), second)
        }
    }
}

fn int_type(int: Int) -> &'static str {
    match int {
        Int::U8 => "byte",
        Int::U16 => "ushort",
        Int::U32 => "uint",
        Int::U64 => "ulong",
    }
}

fn wasm_type(ty: WasmType) -> &'static str {
    match ty {
        WasmType::I32 => "int",
        WasmType::I64 => "long",
        WasmType::F32 => "float",
        WasmType::F64 => "double",
        WasmType::Pointer => "int",
        WasmType::PointerOrI64 => "long",
        WasmType::Length => "int",
    }
}

fn list_element_info(ty: &Type) -> (usize, &'static str) {
    match ty {
        Type::S8 => (1, "sbyte"),
        Type::S16 => (2, "short"),
        Type::S32 => (4, "int"),
        Type::S64 => (8, "long"),
        Type::U8 => (1, "byte"),
        Type::U16 => (2, "ushort"),
        Type::U32 => (4, "uint"),
        Type::U64 => (8, "ulong"),
        Type::F32 => (4, "float"),
        Type::F64 => (8, "double"),
        _ => unreachable!(),
    }
}

fn indent(code: &str) -> String {
    let mut indented = String::with_capacity(code.len());
    let mut indent = 0;
    let mut was_empty = false;
    for line in code.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if was_empty {
                continue;
            }
            was_empty = true;
        } else {
            was_empty = false;
        }

        if trimmed.starts_with('}') {
            indent -= 1;
        }
        indented.extend(iter::repeat(' ').take(indent * 4));
        indented.push_str(trimmed);
        if trimmed.ends_with('{') {
            indent += 1;
        }
        indented.push('\n');
    }
    indented
}

fn interface_name(
    csharp: &mut CSharp,
    resolve: &Resolve,
    name: &WorldKey,
    direction: Direction,
) -> String {
    let pkg = match name {
        WorldKey::Name(_) => None,
        WorldKey::Interface(id) => {
            let pkg = resolve.interfaces[*id].package.unwrap();
            Some(resolve.packages[pkg].name.clone())
        }
    };

    let name = match name {
        WorldKey::Name(name) => name.to_upper_camel_case(),
        WorldKey::Interface(id) => resolve.interfaces[*id]
            .name
            .as_ref()
            .unwrap()
            .to_upper_camel_case(),
    };

    let namespace = match &pkg {
        Some(name) => {
            let mut ns = format!(
                "{}.{}.",
                name.namespace.to_csharp_ident(),
                name.name.to_csharp_ident()
            );

            if let Some(version) = &name.version {
                let v = version
                    .to_string()
                    .replace('.', "_")
                    .replace('-', "_")
                    .replace('+', "_");
                ns = format!("{}v{}.", ns, &v);
            }
            ns
        }
        None => String::new(),
    };

    let world_namespace = &csharp.qualifier();

    format!(
        "{}wit.{}.{}I{name}",
        world_namespace,
        match direction {
            Direction::Import => "imports",
            Direction::Export => "exports",
        },
        namespace
    )
}

fn is_primitive(ty: &Type) -> bool {
    matches!(
        ty,
        Type::U8
            | Type::S8
            | Type::U16
            | Type::S16
            | Type::U32
            | Type::S32
            | Type::U64
            | Type::S64
            | Type::F32
            | Type::F64
    )
}

trait ToCSharpIdent: ToOwned {
    fn to_csharp_ident(&self) -> Self::Owned;
}

impl ToCSharpIdent for str {
    fn to_csharp_ident(&self) -> String {
        // Escape C# keywords
        // Source: https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/keywords/

        //TODO: Repace with actual keywords
        match self {
            "abstract" | "continue" | "for" | "new" | "switch" | "assert" | "default" | "goto"
            | "namespace" | "synchronized" | "boolean" | "do" | "if" | "private" | "this"
            | "break" | "double" | "implements" | "protected" | "throw" | "byte" | "else"
            | "import" | "public" | "throws" | "case" | "enum" | "instanceof" | "return"
            | "transient" | "catch" | "extends" | "int" | "short" | "try" | "char" | "final"
            | "interface" | "static" | "void" | "class" | "finally" | "long" | "strictfp"
            | "volatile" | "const" | "float" | "super" | "while" | "extern" | "sizeof" | "type"
            | "struct" => format!("@{self}"),
            _ => self.to_lower_camel_case(),
        }
    }
}

fn by_resource<'a>(
    funcs: impl Iterator<Item = (&'a str, &'a Function)>,
) -> IndexMap<Option<TypeId>, Vec<&'a Function>> {
    let mut by_resource = IndexMap::<_, Vec<_>>::new();
    for (_, func) in funcs {
        by_resource
            .entry(match &func.kind {
                FunctionKind::Freestanding => None,
                FunctionKind::Method(resource)
                | FunctionKind::Static(resource)
                | FunctionKind::Constructor(resource) => Some(*resource),
            })
            .or_default()
            .push(func);
    }
    by_resource
}

fn dealias(resolve: &Resolve, mut id: TypeId) -> TypeId {
    loop {
        match &resolve.types[id].kind {
            TypeDefKind::Type(Type::Id(that_id)) => id = *that_id,
            _ => break id,
        }
    }
}
