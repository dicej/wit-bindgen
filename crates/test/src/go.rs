use crate::{Compile, LanguageMethods, Runner, Verify};
use anyhow::Result;
use std::borrow::Cow;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use wasm_encoder::{Encode as _, Section as _};
use wit_component::StringEncoding;

pub struct Go;

impl LanguageMethods for Go {
    fn display(&self) -> &str {
        "go"
    }

    fn comment_prefix_for_test_config(&self) -> Option<&str> {
        Some("//@")
    }

    fn should_fail_verify(
        &self,
        _name: &str,
        config: &crate::config::WitConfig,
        _args: &[String],
    ) -> bool {
        config.error_context
    }

    fn codegen_test_variants(&self) -> &[(&str, &[&str])] {
        &[("normal", &[])]
    }

    fn prepare(&self, runner: &mut Runner<'_>) -> Result<()> {
        let cwd = env::current_dir()?;
        let dir = cwd.join(&runner.opts.artifacts).join("go");

        super::write_if_different(&dir.join("test.go"), "package main\n\nfunc main() {}")?;
        super::write_if_different(&dir.join("go.mod"), "module test\n\ngo 1.25")?;

        println!("Testing if `go build` works...");
        runner.run_command(
            Command::new("go")
                .current_dir(&dir)
                .env("GOOS", "wasip1")
                .env("GOARCH", "wasm")
                .arg("build")
                .arg("-buildmode=c-shared")
                .arg("-ldflags=-checklinkname=0"),
        )
    }

    fn compile(&self, runner: &Runner<'_>, compile: &Compile<'_>) -> Result<()> {
        enum CliVersion {
            P2,
            P3,
        }

        let output = compile.output.with_extension("core.wasm");

        // As of this writing, Go-generated modules are not compatible with the
        // `wasi_snapshot_preview1.command` adapter (we get unbounded recursion
        // during initialization), we we must use the
        // `wasi_snapshot_preview1.reactor` one instead.  For `runner.go`, that
        // means we must export `wasi:cli/run` explicitly and thus embed an
        // extra component type custom section to cover that.  For non-async
        // tests, that will be `wasi:cli/run@0.2.6#run`, but for async ones it
        // will be `wasi:cli/run@0.3.0-rc-2025-09-16#run`.

        let cli_version = if let Some(name @ "runner.go") =
            compile.component.path.file_name().unwrap().to_str()
        {
            let runner = fs::read_to_string(&compile.component.path)?;
            fs::write(compile.bindings_dir.join(name), runner.as_bytes())?;
            Some(
                if runner.contains("//go:wasmexport wasi:cli/run@0.2.6#run") {
                    CliVersion::P2
                } else {
                    CliVersion::P3
                },
            )
        } else {
            // Tests which involve importing and/or exporting more than one
            // interface may require more than one file since we can't define
            // more than one package in a single file in Go (AFAICT).  Here we
            // search for files related to `compile.component.path` based on a
            // made-up naming convention.  For example, if the filename is
            // `test.go`, then we'll also include `${prefix}+test.go` for any
            // value of `${prefix}`.
            for path in all_paths(&compile.component.path)? {
                let test = fs::read_to_string(&path)?;
                let package_name = package_name(&test);
                let package_dir = compile.bindings_dir.join(package_name);
                fs::create_dir_all(&package_dir)?;
                fs::write(
                    &package_dir.join(path.file_name().unwrap()),
                    test.as_bytes(),
                )?;
            }
            None
        };

        runner.run_command(
            Command::new("go")
                .current_dir(&compile.bindings_dir)
                .env("GOOS", "wasip1")
                .env("GOARCH", "wasm")
                .arg("build")
                .arg("-o")
                .arg(&output)
                .arg("-buildmode=c-shared")
                .arg("-ldflags=-checklinkname=0"),
        )?;

        if let Some(version) = cli_version {
            let mut resolve = wit_parser::Resolve::default();
            let pkg = resolve.push_str(
                "run.wit",
                match version {
                    CliVersion::P2 => {
                        "package wasi:cli@0.2.6;

interface run {
  run: func() -> result;
}

world command {
  export run;
}"
                    }
                    CliVersion::P3 => {
                        "package wasi:cli@0.3.0-rc-2025-09-16;

interface run {
  run: async func() -> result;
}

world command {
  export run;
}"
                    }
                },
            )?;
            let cli = resolve.select_world(&[pkg], Some(&"command"))?;

            let (pkg, _) = resolve.push_path(&compile.component.bindgen.wit_path)?;
            let test = resolve.select_world(&[pkg], Some(&compile.component.bindgen.world))?;

            let mut module = fs::read(&output)?;
            for (world, name) in [
                (cli, "component-type-wasi-cli"),
                (test, "component-type-test"),
            ] {
                let encoded =
                    wit_component::metadata::encode(&resolve, world, StringEncoding::UTF8, None)?;
                let section = wasm_encoder::CustomSection {
                    name: Cow::Borrowed(name),
                    data: Cow::Borrowed(&encoded),
                };
                module.push(section.id());
                section.encode(&mut module);
            }
            fs::write(&output, &module)?;
        }

        runner.convert_p1_to_component_with_adapter(
            &output,
            compile,
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
        )?;

        Ok(())
    }

    fn verify(&self, runner: &Runner<'_>, verify: &Verify<'_>) -> Result<()> {
        _ = (runner, verify);
        todo!()
    }
}

fn package_name(package: &str) -> &str {
    package
        .split_once('\n')
        .unwrap()
        .0
        .strip_prefix("package ")
        .unwrap()
}

fn all_paths(path: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = vec![path.into()];
    let suffix = ".go";
    if let Some(name) = path
        .file_name()
        .unwrap()
        .to_str()
        .and_then(|name| name.strip_suffix(suffix))
    {
        let suffix = &format!("+{name}{suffix}");
        for entry in path.parent().unwrap().read_dir()? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(suffix))
                .is_some()
            {
                paths.push(entry.path());
            }
        }
    }
    Ok(paths)
}
