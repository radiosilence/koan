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

## One correction at a time

A stack, a profile with layers, may hold only one correction. Two corrections would each try to undo the same headphone, so the colour would be taken out twice and the sound would end up worse than with neither. kōan refuses a second one and says which correction the stack already has. Add your extra changes as a tuning instead.

Each profile's page says which it is, in one line: "Correction: AFUL Performer 8S → Harman in-ear 2019 (from measurement)", then "Tuning: Warm bass".

## Which way for your headphones

1. **Search for your headphones** in **Find in AutoEQ…** under EQ in Settings. If they are there, install them. You have a correction, and you can switch its target on the profile's page.
2. **If they are not in AutoEQ**, choose **Use a measurement instead**. Find your headphones on squig.link, export the measurement as a CSV file, and import it. Then pick in-ear or over-ear, and a target.
3. **If you already have an EQ** made for your headphones, import it and answer "Is this a finished EQ for your headphones, or a tuning to add on top?" with *finished EQ*. Say which target it was made for if you know; if you do not, choose Unknown, and target switching stays off.

Then add any tunings you like on top. See [Equalisation and convolution](dsp.md) for everything else profiles can do.
