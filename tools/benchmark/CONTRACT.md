# Ushas Bench v1 shared contract

Root owns Cargo.toml/lock,src/{main,config,report,compare}.rs,README,CI and git.
Scene owner owns tools/claude-model/**, smoke Claude adapter/Cargo dependency,
and benchmark src/scene.rs. Engine owner owns src/{engine,measurement,capture}.rs.
App owner owns macos/** and package.sh. No agent commits or edits other lanes.

## CLI

Binary `ushas-bench`; commands benchmark,compare,stress,capture.
Common flags: --out DIRECTORY (new path),--mode native|temporal|spatial|bilinear,
--scale 1|2/3|1/2,--width2560,--height1440,--frames1200 (per scene),--seed21434,
--scene materials|geometry|lighting (optional; default all for benchmark/capture),
--duration600 (stress seconds),--claudes N,--lights N,--particles N,--fill N.
Compare adds --rounds1 (quick) or4 (qualification). Use flags as separate argv
elements. Native is always scale1 andMSAA4; reconstructed/bilinear armsMSAAoff.
Preset `standard-v1` means default dimensions/frames/seed and no custom load.
Stress always custom. Renderer mode is fixed at process creation.

Background follow-up: valueless `--background` selects a persistent GPU image
and a schedule-driven runner, independent of native window visibility. CLI
default remains windowed; the app defaults to Background run and can switch to
the visible lab. `RunConfig.background` defaults false when absent in legacy
JSON. Standard background results use `claude-lab-offscreen-v1`; windowed
results retain `claude-lab-standard-v1`. Explicit window presets combined with
`--background` are rejected. Custom results remain custom and retain the flag.
Every comparison child, including its quality replay, inherits the same flag;
different execution targets cannot join a comparison.

Background benchmark/stress never creates a native render window or reads back
images. Keep normal pipelining and the existing cohort completion/proof rules.
Environment evidence names `render_target=offscreen_image`,
`runner=schedule_loop`, `live_preview=false`, and `measured_readbacks=false`.
Only capture replays create screenshot/readback requests. The SwiftUI launcher
remains available for progress, stress controls, Stop and foreground Escape;
background launch/completion must not steal focus from another application.

stdout is newlineJSON events; stderr is diagnostic log. Events have schema_version1,
event:string and optional message,scene,progress,render_fps,report,path. Stable
event names: started,progress,scene_complete,complete,error. The complete event
contains report:absolute result.json path and valid:bool. Unknown events ignored.
Each child writes result.json in its reserved output directory, preserving invalid
and cancelled results. Root writes envelope metadata, hashes and offlineHTML.
Stress accepts stdin JSON lines {"event":"stop"} or {"event":"configure",
"claudes":64,"lights":8,"particles":4096,"fill":0}; configuration starts
a new reporting epoch. Escape stops. No score for cancelled/invalid benchmark.

## Config and engine API (root provides config.rs and report.rs)

Mode enum Native,Temporal,Spatial,Bilinear (serde lowercase). SceneKind enum
Materials,Geometry,Lighting (serde lowercase),ALL array and as_str().
Action enum Benchmark,Stress,Capture (serde lowercase).
RunConfig fields: action:Action,mode:Mode,scale:f32,width:u32,height:u32,
frames:u32,seed:u64,scene:Option<SceneKind>,duration:u64,out:PathBuf,
load:StressLoad. StressLoad fields claudes/lights/particles:Option<u32>,fill:u32.
Defaults2560x1440,1200,21434,600,none scene/load,fill0. Config has validate(),
standard(),scenes(), mode.as_str(),mode.metalfx() mapping (native/bilinearDisabled).

engine::run(config:RunConfig)->EngineResult runs Bevy on main thread and returns
after app exit. Root report types:
SceneResult {scene:String,valid:bool,frames:u32,elapsed_seconds:f64,
render_fps:Option<f64>,errors:Vec<String>};
EngineResult {valid:bool,stopped:bool,errors:Vec<String>,scenes:Vec<SceneResult>,
captures:Vec<serde_json::Value>,stress_samples:Vec<Value>,environment:Value}.
Both derive Serialize/Deserialize/Default/Clone. emit(event:&str,data:Value)
prints oneJSONline with schema_version andevent. Root seals RunReport envelope.

Root result.json fields: schema_version1,kind(command),valid,stopped,errors,
config,profile_version,source_revision,binary_sha256,started_utc,render_fps
(geomean ornull),scenes,captures,stress_samples,environment. Stress render_fps
isnull; periodic samples are completed-render rate, not pacing. Capture entries
must include scene,tick,path and original frame/view identity; image parser can
use scene/tick/path. Comparisons: kind=compare,valid,arms array containing
label,mode,scale,round,report(relative path),valid,render_fps; paired summaries.

## Scene API (scene owner provides scene.rs)

LabScenePlugin; LabCamera component identifies engine-createdCamera3d.
SceneState Resource fields kind:SceneKind,tick:u32,time_seconds:f32,seed:u64,
generation:u64,load:StressLoad,caption:String. Engine inserts before startup.
SceneState derivesClone andDefault. Scene rebuilds only kind/generation/load
changes, never everytick. Every camera/object/shader pose is an analytic function
of state.time_seconds andseed; benchmarktime=tick/120. UI caption is display only.
Engine owns camera target,mode,MSAA,HDR,jitter and reset. Scene owns camera pose,
world geometry/materials/lights/particles plus minimal caption/Escape HUD.

## App

SwiftUI launcher bundles renderer at Contents/Helpers/ushas-bench. Fresh process
per arm; launcher hides during windowed scored rendering and returns for results.
CLI accessible inside.app; noPython/runtime downloads. Baseline defaultnative;
user can choose allfour modes and legal scales. App history stores runs locally;
export produces a self-contained report bundle. Comparison slider uses retained
captures, never starts another renderer. macOS26,arm64,ad-hoc signing only.
