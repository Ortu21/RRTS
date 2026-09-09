# RTS interface

## Surface and intent
Native Bevy desktop HUD in Operate mode. Functional reorganisation of existing game controls, with no new asset or visual-brand dependency.

## Layout
One full-window flex shell: shared top status/economy bar, central battlefield, bottom contextual dock, and an optional 320px debug sidebar. Player dock is 230px high; observer dock is 160px. Help and control menus are normally closed. Dock columns and sidebar scroll internally. Explicit gaps keep panels separate; text and controls remain inside their container when resized.

## Appearance
Use the bundled Bevy font, high-contrast light text, solid dark navy surfaces and blue-grey buttons. Active tools have a lighter teal fill plus a textual [x]; team identity is always named BLUE/RED. Use ASCII separators supported by the bundled font. No gradients, blur, decorative assets or animations.

## Interaction
Click/drag selects controllable units. Inspection is independent and indicated with a neutral ring. Construction, production and orders occupy the same contextual dock. Read-only observation shows entity details and factory queues. Debug opening is distinct from tool activation; closing retains activated tools and the header displays their count. Full debug view has a persistent header label. UI pointer capture lasts through release; scrolling over child buttons affects the nearest scroll container. Camera and UI cadence use real time.
