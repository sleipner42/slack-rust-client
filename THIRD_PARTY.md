Emoji artwork is Twemoji by Twitter, Inc. and other contributors, provided under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
The client bundles the unmodified 72x72 PNG assets from
[Twemoji 17.0.3](https://github.com/jdecked/twemoji/tree/v17.0.3) in
`assets/emoji/twemoji-72.pack` (built with `scripts/build-emoji-pack.py`) and
falls back to the same release on jsDelivr for any emoji it lacks.
Workspace custom emoji are loaded from the URLs supplied by Slack.

Message emphasis uses unmodified Ubuntu Medium by Canonical Ltd., obtained from
[Google Fonts](https://github.com/google/fonts/tree/main/ufl/ubuntu), under the
[Ubuntu Font Licence 1.0](assets/fonts/UFL.txt). The license is also included in
the macOS app bundle under Resources/Licenses.

Spell checking uses unmodified Hunspell dictionaries embedded in the binary:
English (`en_US`) from [SCOWL](https://wordlist.aspell.net) under the terms in
[assets/dictionaries/en_US-README.txt](assets/dictionaries/en_US-README.txt),
and Swedish (`sv_SE`, DSSO 2.40) by Göran Andersson under the
[GNU LGPL 3.0](https://www.gnu.org/licenses/lgpl-3.0.html), see
[assets/dictionaries/sv_SE-README.txt](assets/dictionaries/sv_SE-README.txt).
Both notices are also included in the macOS app bundle under Resources/Licenses.

Huddle audio is encoded with the [Opus codec](https://opus-codec.org), built
from source and linked into the binary, under its
[BSD 3-Clause licence](https://opus-codec.org/license/). The Chime signaling
messages follow `SignalingProtocol.proto` from
[amazon-chime-sdk-js](https://github.com/aws/amazon-chime-sdk-js) (Apache 2.0).
