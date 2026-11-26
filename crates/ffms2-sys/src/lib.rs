//! Raw FFI bindings to `FFMS2` (dynamically linked via `ffms2.dll`).

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::missing_safety_doc
)]

use std::os::raw::{c_char, c_double, c_int, c_uint, c_void};

pub const FFMS_ERROR_SUCCESS: c_int = 0;

pub const FFMS_SEEK_LINEAR_NO_RW: c_int = -1;
pub const FFMS_SEEK_LINEAR: c_int = 0;
pub const FFMS_SEEK_NORMAL: c_int = 1;
pub const FFMS_SEEK_UNSAFE: c_int = 2;
pub const FFMS_SEEK_AGGRESSIVE: c_int = 3;

pub const FFMS_IEH_ABORT: c_int = 0;
pub const FFMS_IEH_CLEAR_TRACK: c_int = 1;
pub const FFMS_IEH_STOP_TRACK: c_int = 2;
pub const FFMS_IEH_IGNORE: c_int = 3;

pub const FFMS_TYPE_UNKNOWN: c_int = -1;
pub const FFMS_TYPE_VIDEO: c_int = 0;
pub const FFMS_TYPE_AUDIO: c_int = 1;
pub const FFMS_TYPE_DATA: c_int = 2;
pub const FFMS_TYPE_SUBTITLE: c_int = 3;
pub const FFMS_TYPE_ATTACHMENT: c_int = 4;

pub const FFMS_RESIZER_FAST_BILINEAR: c_int = 0x0001;
pub const FFMS_RESIZER_BILINEAR: c_int = 0x0002;
pub const FFMS_RESIZER_BICUBIC: c_int = 0x0004;
pub const FFMS_RESIZER_X: c_int = 0x0008;
pub const FFMS_RESIZER_POINT: c_int = 0x0010;
pub const FFMS_RESIZER_AREA: c_int = 0x0020;
pub const FFMS_RESIZER_BICUBLIN: c_int = 0x0040;
pub const FFMS_RESIZER_GAUSS: c_int = 0x0080;
pub const FFMS_RESIZER_SINC: c_int = 0x0100;
pub const FFMS_RESIZER_LANCZOS: c_int = 0x0200;
pub const FFMS_RESIZER_SPLINE: c_int = 0x0400;

pub const FFMS_LOG_QUIET: c_int = -8;
pub const FFMS_LOG_ERROR: c_int = 16;
pub const FFMS_LOG_WARNING: c_int = 24;

#[repr(C)]
pub struct FFMS_ErrorInfo {
    pub ErrorType: c_int,
    pub SubType: c_int,
    pub BufferSize: c_int,
    pub Buffer: *mut c_char,
}

#[repr(C)]
pub struct FFMS_VideoSource {
    _private: [u8; 0],
}

#[repr(C)]
pub struct FFMS_AudioSource {
    _private: [u8; 0],
}

#[repr(C)]
pub struct FFMS_Indexer {
    _private: [u8; 0],
}

#[repr(C)]
pub struct FFMS_Index {
    _private: [u8; 0],
}

#[repr(C)]
pub struct FFMS_Track {
    _private: [u8; 0],
}

#[repr(C)]
pub struct FFMS_Frame {
    pub Data: [*const u8; 4],
    pub Linesize: [c_int; 4],
    pub EncodedWidth: c_int,
    pub EncodedHeight: c_int,
    pub EncodedPixelFormat: c_int,
    pub ScaledWidth: c_int,
    pub ScaledHeight: c_int,
    pub ConvertedPixelFormat: c_int,
    pub KeyFrame: c_int,
    pub RepeatPict: c_int,
    pub InterlacedFrame: c_int,
    pub TopFieldFirst: c_int,
    pub PictType: c_char,
    pub ColorSpace: c_int,
    pub ColorRange: c_int,
    pub ColorPrimaries: c_int,
    pub TransferCharateristics: c_int,
    pub ChromaLocation: c_int,
    pub HasMasteringDisplayPrimaries: c_int,
    pub MasteringDisplayPrimariesX: [c_double; 3],
    pub MasteringDisplayPrimariesY: [c_double; 3],
    pub MasteringDisplayWhitePointX: c_double,
    pub MasteringDisplayWhitePointY: c_double,
    pub HasMasteringDisplayLuminance: c_int,
    pub MasteringDisplayMinLuminance: c_double,
    pub MasteringDisplayMaxLuminance: c_double,
    pub HasContentLightLevel: c_int,
    pub ContentLightLevelMax: c_uint,
    pub ContentLightLevelAverage: c_uint,
    pub DolbyVisionRPU: *mut u8,
    pub DolbyVisionRPUSize: c_int,
    pub HDR10Plus: *mut u8,
    pub HDR10PlusSize: c_int,
    pub LeftEyeData: [*const u8; 4],
    pub LeftEyeLinesize: [c_int; 4],
    pub RightEyeData: [*const u8; 4],
    pub RightEyeLinesize: [c_int; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FFMS_TrackTimeBase {
    pub Num: i64,
    pub Den: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FFMS_FrameInfo {
    pub PTS: i64,
    pub RepeatPict: c_int,
    pub KeyFrame: c_int,
    pub OriginalPTS: i64,
}

#[repr(C)]
pub struct FFMS_VideoProperties {
    pub FPSDenominator: c_int,
    pub FPSNumerator: c_int,
    pub RFFDenominator: c_int,
    pub RFFNumerator: c_int,
    pub NumFrames: c_int,
    pub SARNum: c_int,
    pub SARDen: c_int,
    pub CropTop: c_int,
    pub CropBottom: c_int,
    pub CropLeft: c_int,
    pub CropRight: c_int,
    pub TopFieldFirst: c_int,
    pub ColorSpace: c_int,
    pub ColorRange: c_int,
    pub FirstTime: c_double,
    pub LastTime: c_double,
    pub Rotation: c_int,
    pub Stereo3DType: c_int,
    pub Stereo3DFlags: c_int,
    pub LastEndTime: c_double,
    pub HasMasteringDisplayPrimaries: c_int,
    pub MasteringDisplayPrimariesX: [c_double; 3],
    pub MasteringDisplayPrimariesY: [c_double; 3],
    pub MasteringDisplayWhitePointX: c_double,
    pub MasteringDisplayWhitePointY: c_double,
    pub HasMasteringDisplayLuminance: c_int,
    pub MasteringDisplayMinLuminance: c_double,
    pub MasteringDisplayMaxLuminance: c_double,
    pub HasContentLightLevel: c_int,
    pub ContentLightLevelMax: c_uint,
    pub ContentLightLevelAverage: c_uint,
    pub Flip: c_int,
    pub LastEndPTS: i64,
}

pub type TIndexCallback =
    Option<unsafe extern "C" fn(current: i64, total: i64, ic_private: *mut c_void) -> c_int>;

extern "C" {
    pub fn FFMS_Init(unused: c_int, use_utf8_paths: c_int);
    pub fn FFMS_GetVersion() -> c_int;
    pub fn FFMS_SetLogLevel(level: c_int);
    pub fn FFMS_CreateIndexer(
        source_file: *const c_char,
        error_info: *mut FFMS_ErrorInfo,
    ) -> *mut FFMS_Indexer;
    pub fn FFMS_SetProgressCallback(
        indexer: *mut FFMS_Indexer,
        ic: TIndexCallback,
        ic_private: *mut c_void,
    );
    pub fn FFMS_TrackTypeIndexSettings(
        indexer: *mut FFMS_Indexer,
        track_type: c_int,
        index: c_int,
        unused: c_int,
    );
    pub fn FFMS_DoIndexing2(
        indexer: *mut FFMS_Indexer,
        error_handling: c_int,
        error_info: *mut FFMS_ErrorInfo,
    ) -> *mut FFMS_Index;
    pub fn FFMS_CancelIndexing(indexer: *mut FFMS_Indexer);
    pub fn FFMS_DestroyIndex(index: *mut FFMS_Index);
    pub fn FFMS_ReadIndex(
        index_file: *const c_char,
        error_info: *mut FFMS_ErrorInfo,
    ) -> *mut FFMS_Index;
    pub fn FFMS_WriteIndex(
        index_file: *const c_char,
        index: *mut FFMS_Index,
        error_info: *mut FFMS_ErrorInfo,
    ) -> c_int;
    pub fn FFMS_IndexBelongsToFile(
        index: *mut FFMS_Index,
        source_file: *const c_char,
        error_info: *mut FFMS_ErrorInfo,
    ) -> c_int;
    pub fn FFMS_GetFirstTrackOfType(
        index: *mut FFMS_Index,
        track_type: c_int,
        error_info: *mut FFMS_ErrorInfo,
    ) -> c_int;
    pub fn FFMS_CreateVideoSource(
        source_file: *const c_char,
        track: c_int,
        index: *mut FFMS_Index,
        threads: c_int,
        seek_mode: c_int,
        error_info: *mut FFMS_ErrorInfo,
    ) -> *mut FFMS_VideoSource;
    pub fn FFMS_DestroyVideoSource(v: *mut FFMS_VideoSource);
    pub fn FFMS_GetVideoProperties(v: *mut FFMS_VideoSource) -> *const FFMS_VideoProperties;
    pub fn FFMS_GetFrame(
        v: *mut FFMS_VideoSource,
        n: c_int,
        error_info: *mut FFMS_ErrorInfo,
    ) -> *const FFMS_Frame;
    pub fn FFMS_SetOutputFormatV2(
        v: *mut FFMS_VideoSource,
        target_formats: *const c_int,
        width: c_int,
        height: c_int,
        resizer: c_int,
        error_info: *mut FFMS_ErrorInfo,
    ) -> c_int;
    pub fn FFMS_GetTrackFromVideo(v: *mut FFMS_VideoSource) -> *mut FFMS_Track;
    pub fn FFMS_GetNumFrames(t: *mut FFMS_Track) -> c_int;
    pub fn FFMS_GetFrameInfo(t: *mut FFMS_Track, frame: c_int) -> *const FFMS_FrameInfo;
    pub fn FFMS_GetTimeBase(t: *mut FFMS_Track) -> *const FFMS_TrackTimeBase;
    pub fn FFMS_GetPixFmt(name: *const c_char) -> c_int;
}
