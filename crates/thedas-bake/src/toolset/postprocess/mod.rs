mod atlas;
mod bc3_alpha;
mod bc3_rgb;
mod combine;
mod image;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use thedas_python25::{Error, ProcessOutput, Runtime};

use self::image::Image;

/// Use native implementations of the three subprocesses invoked by LightMapper.py.
pub fn install_postprocessors(runtime: &mut Runtime) {
    runtime.set_process_runner(|command, _| run_postprocessor(command));
}

/// Execute a toolset postprocessor command using native code. Stage failures
/// retain a nonzero child status and diagnostic output for the Python driver.
pub fn run_postprocessor(command: &[String]) -> thedas_python25::Result<ProcessOutput> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| Error::new("ValueError", "empty postprocessor command"))?;
    let command = program.rsplit(['\\', '/']).next().unwrap_or(program);
    let result = match command.to_ascii_lowercase().as_str() {
        "bakedmapprocessor.exe" => combine_command(arguments),
        "createatlas.exe" => atlas_command(arguments),
        "textureprocessor.exe" => texture_command(arguments),
        _ => {
            return Err(Error::unsupported(format!(
                "lightmapper subprocess {command}"
            )));
        }
    };
    let (status, stdout) = match result {
        Ok(()) => (0, Vec::new()),
        Err(error) => (1, format!("{command}: {error:#}\n").into_bytes()),
    };
    Ok(ProcessOutput {
        status,
        stdout,
        stderr: None,
    })
}

fn combine_command(arguments: &[String]) -> Result<()> {
    let args = Options::parse(arguments, &["-in_lm", "-in_sm", "-in_ao", "-out"], &[])?;
    combine::run(
        args.path("-in_lm")?,
        args.path("-in_sm")?,
        args.path("-in_ao")?,
        args.path("-out")?,
    )
}

fn atlas_command(arguments: &[String]) -> Result<()> {
    let args = Options::parse(
        arguments,
        &[
            "-in",
            "-out",
            "-width",
            "-height",
            "-gutter",
            "-in_width",
            "-in_height",
            "-file",
        ],
        &["-halftexel"],
    )?;
    let options = atlas::Options {
        width: args.number("-width", 1024)?,
        height: args.number("-height", 1024)?,
        gutter: args.number("-gutter", 0)?,
        half_texel: args.flags.contains("-halftexel"),
        input_width: args.number("-in_width", 512)?,
        input_height: args.number("-in_height", 512)?,
    };
    atlas::run(
        args.path("-in")?,
        args.path("-out")?,
        args.path("-file")?,
        &options,
    )
}

fn texture_command(arguments: &[String]) -> Result<()> {
    let args = Options::parse(
        arguments,
        &["-input_directory", "-output_directory"],
        &["-dxt5", "-nomipmap"],
    )?;
    ensure!(
        args.flags.contains("-dxt5") && args.flags.contains("-nomipmap"),
        "only the Toolset's DXT5 single-mip output is supported"
    );
    let input = args.path("-input_directory")?;
    let output = args.path("-output_directory")?;
    fs::create_dir_all(output)?;
    for entry in fs::read_dir(input).with_context(|| format!("reading {}", input.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        let extension = path.extension().unwrap_or_default();
        if extension.eq_ignore_ascii_case("meta") {
            continue;
        }
        ensure!(
            extension.eq_ignore_ascii_case("dds"),
            "unsupported atlas texture {}",
            path.display()
        );
        Image::read_dds(&path)?.write_bc3_dds(&output.join(entry.file_name()))?;
    }
    Ok(())
}

struct Options<'a> {
    values: BTreeMap<String, &'a str>,
    flags: BTreeSet<String>,
}

impl<'a> Options<'a> {
    fn parse(arguments: &'a [String], value_keys: &[&str], flag_keys: &[&str]) -> Result<Self> {
        let mut result = Self {
            values: BTreeMap::new(),
            flags: BTreeSet::new(),
        };
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            let key = argument.to_ascii_lowercase();
            if flag_keys.contains(&key.as_str()) {
                result.flags.insert(key);
            } else if value_keys.contains(&key.as_str()) {
                let value = arguments
                    .next()
                    .with_context(|| format!("missing value for {argument}"))?;
                result.values.insert(key, value);
            } else {
                anyhow::bail!("unsupported option {argument}");
            }
        }
        Ok(result)
    }

    fn path(&self, key: &str) -> Result<&Path> {
        self.values
            .get(key)
            .filter(|value| !value.is_empty())
            .map(Path::new)
            .with_context(|| format!("missing required option {key}"))
    }

    fn number(&self, key: &str, default: u32) -> Result<u32> {
        self.values.get(key).map_or(Ok(default), |value| {
            value
                .parse()
                .with_context(|| format!("invalid value for {key}: {value}"))
        })
    }
}

pub(super) fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn write_texture_meta(
    path: &Path,
    width: u32,
    height: u32,
    producer: &str,
) -> Result<()> {
    let (category, texture_type) = if producer == "CreateAtlas" {
        ("LightmapAtlas", "Default")
    } else {
        ("Static", "Lightmap")
    };
    let metadata = format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<MetaData MetaDataVersion=\"1\">\n",
            "  <ModificationTrackingInformation>\n",
            "    <lastExportedFrom><Value><![CDATA[{}]]></Value></lastExportedFrom>\n",
            "  </ModificationTrackingInformation>\n",
            "  <TextureInformation>\n",
            "    <SourceWidth><Value><![CDATA[{}]]></Value></SourceWidth>\n",
            "    <SourceHeight><Value><![CDATA[{}]]></Value></SourceHeight>\n",
            "    <BitsPerPixel><Value><![CDATA[32]]></Value></BitsPerPixel>\n",
            "    <Category><Value><![CDATA[{}]]></Value></Category>\n",
            "    <Type><Value><![CDATA[{}]]></Value></Type>\n",
            "  </TextureInformation>\n",
            "</MetaData>\n",
        ),
        producer, width, height, category, texture_type
    );
    fs::write(path.with_extension("dds.meta"), metadata)?;
    Ok(())
}
