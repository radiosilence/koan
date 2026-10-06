# Headphone EQ, explained

Every headphone colours the sound in its own way: some boom, some hiss, most do a bit of both. Headphone EQ undoes that colour and replaces it with a sound you choose. This page explains the three words the rest of kōan uses for it, and which way to go for your headphones.

## Three words

**A measurement** is a graph of how loud a headphone plays each pitch, from deep bass to high treble, when it is fed the same level at every pitch. People who measure headphones on a test rig publish these. squig.link has thousands, and AutoEQ uses them too.

**A target** is the sound you want instead: the same kind of graph, drawn by researchers or reviewers from what listeners prefer. The Harman targets are the best known. Each target in kōan has a one-line description of how it sounds.

**A correction** is the difference between the two: the target minus the measurement. Where your headphone is too loud, the correction turns it down, and where it is too quiet, the correction turns it up. The result is your headphone sounding like the target.

For example, at three pitches:

| | Measurement | Target | Correction |
|---|---|---|---|
| Bass, 50 Hz | +6 dB | +3 dB | −3 dB |
| Middle, 1 kHz | 0 dB | 0 dB | 0 dB |
| Treble, 8 kHz | −4 dB | +1 dB | +5 dB |

## Two ways to correct a headphone

**A ready-made EQ.** Someone has already worked out the correction for your headphone, as a list of EQ bands: AutoEQ's results, or an EQ exported from squig.link or autoeq.app. It is finished as it is. kōan can only switch it to another target if you tell it which target it was made for.

**A measurement and a target.** You give kōan your headphone's measurement and pick a target, and kōan works out the correction. You can pick a different target at any time. AutoEQ installs work this way too, because kōan keeps the measurement AutoEQ used.

Either way, the result is a **correction**. On top of it you can add **tunings**: changes that suit your taste rather than the headphone, such as more bass or a darker treble. A tuning plays after the correction and can be switched on and off.

## Three kinds of profile

Every profile is one of three, shown as a badge wherever profiles are listed:

- **Correction**: makes your headphones neutral, that is, sound like the target. AutoEQ installs and corrections built from a measurement are corrections.
- **Tuning**: your taste, added on top of a correction. Anything that isn't a correction starts as a tuning, and you can change it on the profile's page.
- **Baked**: a correction with a tuning already in it. Most presets named for a sound are baked, such as Qudelix's "Lush" or a squig.link export to a target with extra bass. A baked profile counts as the correction, so adding a tuning on top would add taste twice.

kōan can't tell from the file which an imported EQ is, so it asks.

The target belongs to the correction, and each says what it does against neutral. **Neutral (diffuse field)**, offered first for in-ears and over-ears alike, has no bass or treble preference; Harman's targets are neutral plus the bass and treble most listeners preferred. Moving an AutoEQ correction from Harman to neutral takes Harman's preference out, which leaves room for a tuning of your own on top.

## One correction at a time

A stack, a profile with layers, may hold only one correction, and a baked profile counts as one. Two corrections would each try to undo the same headphone, so the colour would be taken out twice and the sound would end up worse than with neither. kōan refuses a second one and says which correction the stack already has. Add your extra changes as a tuning instead.

A stack made before kōan refused this still plays, and its page says what's wrong: "This stack corrects twice: Cantor (AutoEQ) and Performer 8S (baked). Keep one."

The top of each profile's page says what the chain does, one line per role, each with its badge: "Correction: AFUL Performer 8S → Harman in-ear 2019 (from measurement)", then "Tuning: Warm bass". A baked preset reads "Baked: AFUL Performer 8S Lush (correction + tuning in one)".

## Which way for your headphones

1. **Search for your headphones** in **Find in AutoEQ…** under EQ in Settings. If they are there, install them. You have a correction, and you can switch its target on the profile's page.
2. **If they are not in AutoEQ**, choose **Use a measurement instead** and search for them under **Find it on squig.link**. kōan searches the catalogues of the reviewers' squig.link sites, each kept for a day, and lists every measurement of that model with its site, its rig where the site says, and its variant: tips, inserts, a port. Pick one measured on the rig your target assumes. kōan fetches its left and right channels, averages them, and credits the site on the profile. A measurement exported from squig.link or REW as a CSV file works too. Then pick in-ear or over-ear, and a target.
3. **If you already have an EQ** made for your headphones, import it and answer "What is this EQ?". Choose *A neutral correction for these headphones* if it only corrects them, or *A correction with a sound already in it* if it's named for a sound or adds bass of its own. For a neutral correction, say which target it was made for if you know; if you don't, choose Unknown, and target switching stays off.

Then add any tunings you like on top. See [Equalisation and convolution](dsp.md) for everything else profiles can do.
