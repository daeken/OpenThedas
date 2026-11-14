# Deferred coverage for authored blended cutouts

The Origins material/state resources and shader operations were inspected locally.
This note records the resulting coverage rules; no game resource or shader dump is
included in the repository.

`Prop1/Prop1_Punch` uses a diffuse texture and opacity multiplier. It clips their
product below 0.3 and then applies the state alpha comparison. `PropPunch` also enables
ordinary source-alpha colour blending and depth writing. Its missing alpha reference
uses the state parser's default of zero. This differs from the opaque Static1 punch
path, despite the shared punch name.

`Eye0/Eyelash0` uses diffuse alpha times opacity. `EyelashPnch` enables the same colour
blend and depth writing, with a greater-than comparison and reference 20 in the
inspected resource. The shadow planner reads the actual comparison/reference rather
than hard-coding those defaults.

The earlier cutout planner recognized eyelashes but rejected every blended pass, so
the authored eyelash state never reached its opacity sampler. The planner now accepts
only these two known families with their source-alpha/inverse-source-alpha additive
blend, an alpha test, depth enabled and depth writes. Their visible colour equations
remain in the forward renderer. Other blends and unknown materials stay excluded;
the colour-disabled hair depth core retains its stricter existing rule.

Point-shadow cubes represent binary blocking at surviving texels. They preserve the
sampled holes and tests, but do not model fractional light transmission through the
remaining partially opaque texels. Missing maps, no-cast flags, runtime fades,
cutaway, LOD and unsupported palettes retain their existing exclusions.
