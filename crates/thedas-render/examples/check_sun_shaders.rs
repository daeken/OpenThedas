//! Read-only check of an external shader archive; no game data is written or embedded.
//! Run with a scratch HOME: cargo run -p thedas-render --example check_sun_shaders -- <shaders.erf>
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("give a shaders.erf path")?;
    let erf = thedas_vfs::Erf::open(path)?;
    let mut checked = 0;
    let mut receivers = 0;
    let mut unmatched = 0;
    for entry in &erf.entries {
        let name = entry.display_name();
        if !name.ends_with(".fxo") {
            continue;
        }
        let shader = thedas_shader::Shader::parse(&erf.read(entry, None)?)?;
        let translated = thedas_shader::translate(
            &shader,
            &thedas_shader::Options {
                soft_shadows: true,
                ..Default::default()
            },
        )
        .map_err(|e| format!("{name}: {e}"))?;
        thedas_shader::backends::check_wgsl(&translated.wgsl)
            .map_err(|e| format!("{name}: {e}"))?;
        if translated
            .wgsl
            .contains("soft_shadow_visibility(soft_shadow_world")
        {
            println!("improved receiver: {name}");
            receivers += 1;
        } else if shader.ctab.as_ref().is_some_and(|t| {
            [
                "g_vShadowData",
                "smpShadowPartition1",
                "smpShadowPartition2",
            ]
            .iter()
            .all(|name| t.constants.iter().any(|c| c.name == *name))
        }) {
            println!("unchanged dual receiver: {name}");
            unmatched += 1;
        }
        checked += 1;
    }
    println!("{checked} shaders checked across backends; {receivers} improved receivers");
    if receivers == 0 || unmatched != 0 {
        return Err(format!(
            "receiver coverage incomplete: {receivers} recognized, {unmatched} unmatched"
        )
        .into());
    }
    Ok(())
}
