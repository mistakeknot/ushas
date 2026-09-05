# Benchmark character

The Claude subject is an independent procedural 3D interpretation of the happy
Claude illustration by **vgel / thebes**, referenced from
[vgel's Redbubble shop](https://www.redbubble.com/people/vgel/shop).
The specific reference is
[happy claude, free from trolley problems](https://www.redbubble.com/i/sticker/happy-claude-free-from-trolley-problems-by-vgel/167765510/djes).
It is not an artist-provided mesh. The shop artwork is a visual reference;
no copy of that image or an artwork texture is bundled with this fixture.

The model keeps the irregular coral sunburst, white oval face, happy black
eyes and W mouth, lavender sleeves, blue trousers, brown shoes, and curved
orange tail. Every visible part is solid geometry with an opaque, lit Bevy
material, so it participates in depth and motion-vector prepasses.

`claude-toy-v1` identifies this geometry in smoke reports. The face points
along local +Z and the feet rest at local y=0. Moving runs articulate the
head, arms, legs, and tail and turn the whole subject; static runs retain
the authored pose. The old shapes subject remains a separate historical
comparison, rather than sharing results with this geometry.
