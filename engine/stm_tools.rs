pub fn calculate_stm_tempo(tempo: u8) -> u8 {
    // based on OpenMPT's way of calculating it
    let mut st2_mixing_rate: isize = 23863;
    let tempo_table: [u8; 16] = [140, 50, 25, 15, 10, 7, 6, 4, 3, 3, 2, 2, 2, 2, 1, 1];
    let samples_per_tick = st2_mixing_rate
        / (50 - ((tempo_table[(tempo as usize) >> 4] * (tempo & 0x0f)) >> 4) as isize);
    st2_mixing_rate *= 5;
    st2_mixing_rate += samples_per_tick;
    st2_mixing_rate = if st2_mixing_rate >= 0 {
        st2_mixing_rate / (samples_per_tick * 2)
    } else {
        (st2_mixing_rate - ((samples_per_tick * 2) - 1)) / (samples_per_tick * 2)
    };
    st2_mixing_rate as u8
}
