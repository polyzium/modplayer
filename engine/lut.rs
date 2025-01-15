use std::sync::LazyLock;

use fixed::types::{U0F32, U32F32};

pub const LINEAR_UP: LazyLock<[U32F32;256]> = LazyLock::new(||
    std::array::from_fn(|index| U32F32::from_num(2f32.powf(index as f32/192.0)))
);

pub const LINEAR_DOWN: LazyLock<[U0F32;256]> = LazyLock::new(||
    std::array::from_fn(|index| U0F32::from_num(2f32.powf(-(index as f32)/192.0)))
);

pub const FINE_LINEAR_UP: LazyLock<[U32F32;16]> = LazyLock::new(||
    std::array::from_fn(|index| U32F32::from_num(2f32.powf(index as f32/768.0)))
);

pub const FINE_LINEAR_DOWN: LazyLock<[U0F32;16]> = LazyLock::new(||
    std::array::from_fn(|index| U0F32::from_num(2f32.powf(-(index as f32)/768.0)))
);

pub const PITCH_TABLE: LazyLock<[U32F32; 128]> = LazyLock::new(||
    std::array::from_fn(|index| U32F32::from_num(
        2f32.powf((index as f32 - 60.0) / 12.0)
    ))
);