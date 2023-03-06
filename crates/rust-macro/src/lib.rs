use proc_macro2::{Span, TokenStream};
use std::fs;
use std::path::{Path, PathBuf};
use syn::parse::{Error, Parse, ParseStream, Result};
use syn::punctuated::Punctuated;
use syn::{token, Token};
use wit_bindgen_core::wit_parser::{self, PackageId, Resolve, UnresolvedPackage, WorldId};
use wit_bindgen_rust::Opts;

#[proc_macro]
pub fn generate(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    syn::parse_macro_input!(input as Config)
        .expand()
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

struct Config {
    opts: Opts,
    resolve: Resolve,
    world: WorldId,
    files: Vec<PathBuf>,
}

enum Source {
    Path(String),
    Inline(String),
}

impl Parse for Config {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let call_site = Span::call_site();
        let mut opts = Opts::default();
        let mut world = None;
        let mut source = None;
        let mut substitutions = None;

        if input.peek(token::Brace) {
            let content;
            syn::braced!(content in input);
            let fields = Punctuated::<Opt, Token![,]>::parse_terminated(&content)?;
            for field in fields.into_pairs() {
                match field.into_value() {
                    Opt::Path(s) => {
                        if source.is_some() {
                            return Err(Error::new(s.span(), "cannot specify second source"));
                        }
                        source = Some(Source::Path(s.value()));
                    }
                    Opt::World(s) => {
                        if world.is_some() {
                            return Err(Error::new(s.span(), "cannot specify second world"));
                        }
                        world = Some(s.value());
                    }
                    Opt::Inline(s) => {
                        if source.is_some() {
                            return Err(Error::new(s.span(), "cannot specify second source"));
                        }
                        source = Some(Source::Inline(s.value()));
                    }
                    Opt::SubstitutionsPath(s) => {
                        if substitutions.is_some() {
                            return Err(Error::new(
                                s.span(),
                                "cannot specify second substitutions",
                            ));
                        }
                        substitutions = Some(Source::Path(s.value()));
                    }
                    Opt::SubstitutionsInline(s) => {
                        if substitutions.is_some() {
                            return Err(Error::new(
                                s.span(),
                                "cannot specify second substitutions",
                            ));
                        }
                        substitutions = Some(Source::Inline(s.value()));
                    }
                    Opt::UseStdFeature => opts.std_feature = true,
                    Opt::RawStrings => opts.raw_strings = true,
                    Opt::MacroExport => opts.macro_export = true,
                    Opt::MacroCallPrefix(prefix) => opts.macro_call_prefix = Some(prefix.value()),
                    Opt::ExportMacroName(name) => opts.export_macro_name = Some(name.value()),
                    Opt::Skip(list) => opts.skip.extend(list.iter().map(|i| i.value())),
                }
            }
        } else {
            world = input.parse::<Option<syn::LitStr>>()?.map(|s| s.value());
            if input.parse::<Option<syn::token::In>>()?.is_some() {
                source = Some(Source::Path(input.parse::<syn::LitStr>()?.value()));
            }
        }
        let (resolve, pkg, files) = parse_source(&source, &substitutions, world.as_deref())
            .map_err(|err| Error::new(call_site, format!("{err:?}")))?;
        let world = resolve
            .select_world(pkg, world.as_deref())
            .map_err(|e| Error::new(call_site, format!("{e:?}")))?;
        Ok(Config {
            opts,
            resolve,
            world,
            files,
        })
    }
}

fn parse_source(
    source: &Option<Source>,
    substitutions: &Option<Source>,
    world_name: Option<&str>,
) -> anyhow::Result<(Resolve, PackageId, Vec<PathBuf>)> {
    let mut resolve = Resolve::default();
    let mut files = Vec::new();
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut parse = |path: &Path| -> anyhow::Result<_> {
        if path.is_dir() {
            let (pkg, sources) = resolve.push_dir(&path)?;
            files = sources;
            Ok(pkg)
        } else {
            let pkg = UnresolvedPackage::parse_file(path)?;
            files.extend(pkg.source_files().map(|s| s.to_owned()));
            resolve.push(pkg, &Default::default())
        }
    };
    let pkg = match source {
        Some(Source::Inline(s)) => resolve.push(
            UnresolvedPackage::parse("macro-input".as_ref(), &s)?,
            &Default::default(),
        )?,
        Some(Source::Path(s)) => parse(&root.join(&s))?,
        None => parse(&root.join("wit"))?,
    };
    match substitutions {
        Some(Source::Inline(s)) => {
            wit_parser::expand(&mut resolve, pkg, world_name, toml::from_str(s)?)?
        }
        Some(Source::Path(s)) => wit_parser::expand(
            &mut resolve,
            pkg,
            world_name,
            toml::from_str(&fs::read_to_string(&root.join(&s))?)?,
        )?,
        None => (),
    }
    Ok((resolve, pkg, files))
}

impl Config {
    fn expand(self) -> Result<TokenStream> {
        let mut files = Default::default();
        self.opts
            .build()
            .generate(&self.resolve, self.world, &mut files);
        let (_, src) = files.iter().next().unwrap();
        let src = std::str::from_utf8(src).unwrap();
        let mut contents = src.parse::<TokenStream>().unwrap();

        // Include a dummy `include_str!` for any files we read so rustc knows that
        // we depend on the contents of those files.
        for file in self.files.iter() {
            contents.extend(
                format!("const _: &str = include_str!(r#\"{}\"#);\n", file.display())
                    .parse::<TokenStream>()
                    .unwrap(),
            );
        }

        Ok(contents)
    }
}

mod kw {
    syn::custom_keyword!(std_feature);
    syn::custom_keyword!(raw_strings);
    syn::custom_keyword!(macro_export);
    syn::custom_keyword!(macro_call_prefix);
    syn::custom_keyword!(export_macro_name);
    syn::custom_keyword!(skip);
    syn::custom_keyword!(world);
    syn::custom_keyword!(path);
    syn::custom_keyword!(inline);
    syn::custom_keyword!(substitutions_path);
    syn::custom_keyword!(substitutions_inline);
}

enum Opt {
    World(syn::LitStr),
    Path(syn::LitStr),
    Inline(syn::LitStr),
    SubstitutionsPath(syn::LitStr),
    SubstitutionsInline(syn::LitStr),
    UseStdFeature,
    RawStrings,
    MacroExport,
    MacroCallPrefix(syn::LitStr),
    ExportMacroName(syn::LitStr),
    Skip(Vec<syn::LitStr>),
}

impl Parse for Opt {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let l = input.lookahead1();
        if l.peek(kw::path) {
            input.parse::<kw::path>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Path(input.parse()?))
        } else if l.peek(kw::inline) {
            input.parse::<kw::inline>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::Inline(input.parse()?))
        } else if l.peek(kw::substitutions_path) {
            input.parse::<kw::substitutions_path>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::SubstitutionsPath(input.parse()?))
        } else if l.peek(kw::substitutions_inline) {
            input.parse::<kw::substitutions_inline>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::SubstitutionsInline(input.parse()?))
        } else if l.peek(kw::world) {
            input.parse::<kw::world>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::World(input.parse()?))
        } else if l.peek(kw::std_feature) {
            input.parse::<kw::std_feature>()?;
            Ok(Opt::UseStdFeature)
        } else if l.peek(kw::raw_strings) {
            input.parse::<kw::raw_strings>()?;
            Ok(Opt::RawStrings)
        } else if l.peek(kw::macro_export) {
            input.parse::<kw::macro_export>()?;
            Ok(Opt::MacroExport)
        } else if l.peek(kw::macro_call_prefix) {
            input.parse::<kw::macro_call_prefix>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::MacroCallPrefix(input.parse()?))
        } else if l.peek(kw::export_macro_name) {
            input.parse::<kw::export_macro_name>()?;
            input.parse::<Token![:]>()?;
            Ok(Opt::ExportMacroName(input.parse()?))
        } else if l.peek(kw::skip) {
            input.parse::<kw::skip>()?;
            input.parse::<Token![:]>()?;
            let contents;
            syn::bracketed!(contents in input);
            let list = Punctuated::<_, Token![,]>::parse_terminated(&contents)?;
            Ok(Opt::Skip(list.iter().cloned().collect()))
        } else {
            Err(l.error())
        }
    }
}
