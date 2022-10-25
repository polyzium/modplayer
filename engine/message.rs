/*!
  Send player messages between threads; used to handle SDLCallback events.
  */

#[derive(PartialEq, Eq, Debug)]
pub enum Message {
    Stop,
    Pause,
    Resume,
}

