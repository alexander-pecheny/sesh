#import "sesh.h"

// libghostty's header declares ghostty_surface_free_text with a surface argument the
// implementation does not take; calling it as declared corrupts the surface.
void sesh_ghostty_free_text(void *) __asm__("_ghostty_surface_free_text");
