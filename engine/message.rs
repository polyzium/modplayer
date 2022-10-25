/*!
Send player messages between threads; used to handle SDLCallback events.
*/

/**
 * A message send to the main thread, from the player thread.
 *
 * In a future version, this will be sent between a Player and a
 * PlayerInterface, irrespective of threading.
 */
#[derive(PartialEq, Eq, Debug)]
pub enum Message {
    Stop,
    Pause,
    Resume,
}
