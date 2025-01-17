use std::{f32::consts::PI, sync::LazyLock};

use fixed::types::{I32F32, U0F32, U32F32};

pub const LINEAR_UP: LazyLock<[U32F32;256]> = LazyLock::new(||
    std::array::from_fn(|index| U32F32::from_num(2f32.powf(index as f32/192.0)))
);

pub const LINEAR_DOWN: LazyLock<[U32F32;256]> = LazyLock::new(||
    std::array::from_fn(|index| {
        return U32F32::saturating_from_num(2f32.powf(-(index as f32)/192.0));
    })
);

pub const FINE_LINEAR_UP: LazyLock<[U32F32;16]> = LazyLock::new(||
    std::array::from_fn(|index| U32F32::from_num(2f32.powf(index as f32/768.0)))
);

pub const FINE_LINEAR_DOWN: LazyLock<[U32F32;16]> = LazyLock::new(||
    std::array::from_fn(|index| U32F32::from_num(2f32.powf(-(index as f32)/768.0)))
);

pub const PITCH_TABLE: LazyLock<[U32F32; 128]> = LazyLock::new(||
    std::array::from_fn(|index| U32F32::from_num(
        2f32.powf((index as f32 - 60.0) / 12.0)
    ))
);

pub const SINC_PRECISION: u8 = 16;
pub const SINC_SIZE: usize = 1024;

pub const SINC: LazyLock<[I32F32; SINC_SIZE]> = LazyLock::new(||
    std::array::from_fn(|x| { let fx = x as f32-512.0; I32F32::from_num(((fx/(SINC_PRECISION as f32)).sin() * PI) / ((fx/(SINC_PRECISION as f32)) * PI)) } )
);

pub fn lut_sinc(x: I32F32) -> I32F32 {
    SINC[x.to_num::<usize>() + 512]
}