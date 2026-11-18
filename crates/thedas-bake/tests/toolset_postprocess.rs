use std::{
    fs,
    path::{Path, PathBuf},
};

use thedas_bake::toolset::run_postprocessor;

#[test]
#[ignore = "requires Wine combination captures and Windows atlas/compression captures"]
fn compare_postprocessor_stages_with_identical_inputs() {
    let reference = PathBuf::from(std::env::var_os("THEDAS_BAKE_POSTPROCESS_REFERENCE").unwrap());
    let output = std::env::temp_dir().join(format!("thedas-postprocess-{}", std::process::id()));
    fs::create_dir(&output).unwrap();
    let output = TempDir(output);

    for case in ["channels", "dimension", "near-constant", "threshold"] {
        let fixture = reference.join("combine-probes").join(case);
        let result = output.0.join(case);
        let maps = fixture.join("maps");
        run(
            "BakedMapProcessor.exe",
            &[
                ("-in_lm", &maps),
                ("-in_sm", &maps),
                ("-in_ao", &maps),
                ("-out", &result),
            ],
            &[],
        );
        exact(&result.join("lmcase.dds"), &fixture.join("out/lmcase.dds"));
        compare_meta(
            &result.join("lmcase.dds.meta"),
            &fixture.join("out/lmcase.dds.meta"),
        );
    }

    for (case, size, pages) in [("terrain", "1024", 1), ("pack", "16", 7)] {
        let fixture = reference.join("windows-atlas-probes").join(case);
        let result = output.0.join(case);
        compare_atlas(&fixture, &result, size, pages);
        let compressed = output.0.join(format!("{case}-compressed"));
        compress(&fixture.join("compress-input"), &compressed);
        let original_compressed = fixture.join("compressed");
        let names = fs::read_dir(&original_compressed)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "dds"))
            .collect::<Vec<_>>();
        assert_eq!(names.len(), pages);
        for name in names {
            exact(&compressed.join(&name), &original_compressed.join(&name));
        }
    }

    for (case, pages) in [("reverse32", 1), ("equal40", 3), ("mixed80", 1)] {
        let fixture = reference.join("windows-atlas-order").join(case);
        compare_atlas(&fixture, &output.0.join(case), "1024", pages);
    }

    if let Some(candidate) = std::env::var_os("THEDAS_BAKE_FULL_RESULT") {
        let candidate = PathBuf::from(candidate);
        let original = reference.join("windows-atlas-probes/terrain");
        println!("Full 32×32 job comparison");
        exact(
            &candidate.join("probe_Room1dat.xml"),
            &original.join("out/probe_Room1dat.xml"),
        );
        exact(
            &candidate.join("probe_Room100.dds"),
            &original.join("compressed/probe_Room100.dds"),
        );
    }
}

#[test]
#[ignore = "requires the original TextureProcessor BC3 block captures"]
fn legacy_bc3_blocks_match_original() {
    let reference = PathBuf::from(std::env::var_os("THEDAS_BAKE_POSTPROCESS_REFERENCE").unwrap())
        .join("compression-probes");
    let output = std::env::temp_dir().join(format!("thedas-bc3-{}", std::process::id()));
    fs::create_dir(&output).unwrap();
    let output = TempDir(output);
    for (input, expected, names) in [
        ("input", "original", &["blocks.dds"][..]),
        (
            "heldout-input",
            "heldout-original",
            &[
                "broad.dds",
                "partial.dds",
                "sparse.dds",
                "range.dds",
                "ramps.dds",
            ],
        ),
        ("edge-input", "edge-original", &["extremes.dds"]),
        (
            "rgb-heldout-input",
            "rgb-heldout-original",
            &["clusters.dds", "grays.dds", "pairs.dds"],
        ),
        ("rgb-gamut-input", "rgb-gamut-original", &["gamut.dds"]),
    ] {
        let result = output.0.join(input);
        compress(&reference.join(input), &result);
        for name in names {
            exact(&result.join(name), &reference.join(expected).join(name));
        }
    }

    let aligned = output.0.join("unaligned");
    compress(&reference.join("unaligned-input"), &aligned);
    let originals: Vec<_> = fs::read_dir(reference.join("unaligned-original"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "dds"))
        .collect();
    assert_eq!(originals.len(), 16);
    for original in originals {
        exact(&aligned.join(original.file_name().unwrap()), &original);
    }
}

#[test]
#[ignore = "requires the captured original editor bake of lgt101d"]
fn editor_job_combination_and_atlas_match_original() {
    let reference = PathBuf::from(std::env::var_os("THEDAS_BAKE_EDITOR_REFERENCE").unwrap());
    let job = PathBuf::from(std::env::var_os("THEDAS_BAKE_EDITOR_JOB").unwrap());
    let output =
        std::env::temp_dir().join(format!("thedas-editor-postprocess-{}", std::process::id()));
    fs::create_dir(&output).unwrap();
    let output = TempDir(output);
    let combined = output.0.join("PreAtlas");
    let atlased = output.0.join("Atlased");
    let films = job.join("TGA");
    run(
        "BakedMapProcessor.exe",
        &[
            ("-in_lm", &films),
            ("-in_sm", &films),
            ("-in_ao", &reference.join("CachedScene/AO")),
            ("-out", &combined),
        ],
        &[],
    );
    compare_dds_directory(&combined, &reference.join("PreAtlas"), 190);

    run(
        "CreateAtlas.exe",
        &[
            ("-in", &combined),
            ("-out", &atlased),
            ("-file", &job.join("job_atlas.xml")),
        ],
        &["-halftexel"],
    );
    compare_dds_directory(&atlased, &job.join("Atlased"), 3);
    let placement = atlased.join("lgt101d_Room0dat.xml");
    let original_atlases = reference.join("CombinedMaps");
    exact(&placement, &original_atlases.join("lgt101d_Room0dat.xml"));
    fs::remove_file(&placement).unwrap();

    let compressed = output.0.join("CombinedMaps");
    compress(&atlased, &compressed);
    for page in 0..3 {
        let name = format!("lgt101d_Room{page:03}.dds");
        exact(&compressed.join(&name), &original_atlases.join(&name));
    }
}

fn compare_dds_directory(candidate: &Path, original: &Path, expected_count: usize) {
    let dds_names = |dir: &Path| {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "dds"))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let expected = dds_names(original);
    assert_eq!(expected.len(), expected_count, "{}", original.display());
    assert_eq!(dds_names(candidate), expected, "{}", candidate.display());
    for name in expected {
        exact(&candidate.join(&name), &original.join(&name));
        let metadata = Path::new(&name).with_extension("dds.meta");
        compare_meta(&candidate.join(&metadata), &original.join(&metadata));
    }
}

fn compress(input: &Path, output: &Path) {
    run(
        "TextureProcessor.exe",
        &[("-input_directory", input), ("-output_directory", output)],
        &["-dxt5", "-nomipmap"],
    );
}

fn compare_atlas(fixture: &Path, result: &Path, size: &str, pages: usize) {
    let width = Path::new(size);
    run(
        "CreateAtlas.exe",
        &[
            ("-in", &fixture.join("in")),
            ("-out", result),
            ("-width", width),
            ("-height", width),
            ("-file", &fixture.join("job.xml")),
        ],
        &["-halftexel"],
    );
    let original_atlas = fixture.join("out");
    let mut dds_count = 0;
    let mut xml_count = 0;
    for entry in fs::read_dir(&original_atlas).unwrap() {
        let name = entry.unwrap().file_name();
        let path = Path::new(&name);
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("dds") => {
                dds_count += 1;
                exact(&result.join(path), &original_atlas.join(path));
                let metadata = path.with_extension("dds.meta");
                compare_meta(&result.join(&metadata), &original_atlas.join(&metadata));
            }
            Some("xml") => {
                xml_count += 1;
                exact(&result.join(path), &original_atlas.join(path));
            }
            _ => {}
        }
    }
    assert_eq!(dds_count, pages, "{}", fixture.display());
    assert_eq!(xml_count, 1, "{}", fixture.display());
    assert_eq!(fs::read_dir(result).unwrap().count(), 2 * pages + 1);
}

fn run(program: &str, paths: &[(&str, &Path)], flags: &[&str]) {
    let mut command = vec![program.to_owned()];
    command.extend(
        paths
            .iter()
            .flat_map(|(key, path)| [key.to_string(), path.to_str().unwrap().to_owned()]),
    );
    command.extend(flags.iter().copied().map(str::to_owned));
    let result = run_postprocessor(&command).unwrap();
    assert_eq!(
        result.status,
        0,
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

fn exact(candidate: &Path, original: &Path) {
    let candidate = fs::read(candidate).unwrap();
    let expected = fs::read(original).unwrap();
    assert_eq!(candidate.len(), expected.len(), "{}", original.display());
    assert_eq!(
        candidate
            .iter()
            .zip(&expected)
            .position(|(left, right)| left != right),
        None,
        "{}",
        original.display()
    );
}

fn compare_meta(candidate: &Path, original: &Path) {
    let candidate = fs::read_to_string(candidate).unwrap();
    // The Wine BakedMapProcessor capture moves the Value tag's closing
    // angle bracket into the CDATA opener.
    let expected = fs::read_to_string(original).unwrap().replace(
        "<Value\r\n\t\t\t\t<![CDATA[>",
        "<Value>\r\n\t\t\t\t<![CDATA[",
    );
    let candidate = roxmltree::Document::parse(candidate.trim_start_matches('\u{feff}')).unwrap();
    let expected = roxmltree::Document::parse(expected.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(
        candidate.root_element().attribute("MetaDataVersion"),
        expected.root_element().attribute("MetaDataVersion"),
    );
    let value = |doc: &roxmltree::Document<'_>, tag| -> String {
        doc.descendants()
            .find(|node| node.has_tag_name(tag))
            .unwrap()
            .children()
            .find(|node| node.has_tag_name("Value"))
            .unwrap()
            .children()
            .filter(|node| node.is_text())
            .filter_map(|node| node.text())
            .collect::<String>()
            .trim()
            .to_owned()
    };
    for tag in [
        "SourceWidth",
        "SourceHeight",
        "BitsPerPixel",
        "Category",
        "Type",
        "lastExportedFrom",
    ] {
        assert_eq!(
            value(&candidate, tag),
            value(&expected, tag),
            "{tag} in {}",
            original.display(),
        );
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
