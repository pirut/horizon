//! An Earth world's window of cubes follows its player (docs/earth.md): when
//! they come within `earthgen::WINDOW_MARGIN` blocks of the window's top or
//! bottom, the level is saved and opened again centred on them, the way a
//! command moves a player to another level, and they wait where they were
//! until the cubes around them are in.
use super::*;

impl Game<'_> {
    pub(super) fn follow_earth_window(&mut self) -> Result<()> {
        // The in-frame game's own player; a server's levels keep their window.
        if self.remote.is_some() || self.travelling.is_some() {
            return Ok(());
        }
        let (Some(earth), Some(player)) = (&self.streaming.earth, &self.player) else {
            return Ok(());
        };
        let y = player.position.y.floor() as i32;
        if earth
            .moved_window(horizon_client::chunk::min_y(), y)
            .is_none()
        {
            return Ok(());
        }
        let Some(player) = self.player.take() else {
            return Ok(());
        };
        let position = player.position;
        if let Err(e) = self.switch_level(self.dim, position) {
            self.player = Some(player);
            return Err(e);
        }
        self.travelling = Some((player, position));
        Ok(())
    }
}
