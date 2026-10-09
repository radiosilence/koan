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

### The treble is left alone

A correction built from a measurement is the one squig.link's Auto EQ would make: kōan fits up to 20 peaking bands between 20 Hz and 6 kHz and a preamp, boosting by no more than 12 dB anywhere (so a measurement with a poor seal is not met with as much bass), by the site's own method, and keeps them as the correction's bands. From the same measurement and target they play what a preset exported from the site plays. Above about 6 kHz a measurement says more about the test rig and how the earphone sat in it than about the headphone: two rigs, or two insertions on one, disagree there by several decibels, and correcting to one of them would add as much error as it took out, so the bands leave the treble alone. A tuning made against a target plays its difference from the correction's target in full up to 6 kHz, fading out by 12 kHz, so a correction to neutral with Harman on top comes close to a correction to Harman. Choosing another target fits the bands again. A correction saved from a measurement by 0.60.6 or earlier is fitted the same way when it plays, so it no longer corrects above about 6 kHz, where those versions applied up to 12 dB to 10 kHz and 3 dB above: on most earphones the treble changes by several decibels.

## Two ways to correct a headphone

**A ready-made EQ.** Someone has already worked out the correction for your headphone, as a list of EQ bands: AutoEQ's results, or an EQ exported from squig.link or autoeq.app. It is finished as it is. kōan can only switch it to another target if you tell it which target it was made for.

**A measurement and a target.** You give kōan your headphone's measurement and pick a target, and kōan works out the correction. You can pick a different target at any time. AutoEQ installs work this way too, because kōan keeps the measurement AutoEQ used.

Either way, the result is a **correction**. On top of it you can add **tunings**: changes that suit your taste rather than the headphone, such as more bass or a darker treble. A tuning plays after the correction and can be switched on and off.

## Three kinds of EQ

Every EQ is one of three, shown as a badge wherever EQs are listed:

- **Correction**: makes your headphones or speakers neutral, that is, sound like the target. AutoEQ installs and corrections built from a measurement are corrections.
- **Tuning**: your taste, added on top of a correction. Anything that isn't a correction starts as a tuning, and you can change it on the EQ's page.
- **Correction with a tuning in it**: a correction that already includes a tuning. Most EQs named for a sound are of this kind, such as Qudelix's "Lush" or a squig.link export to a target with extra bass. Such a correction counts as the correction, so adding a tuning on top would add taste twice.

kōan can't tell from the file which an imported EQ is, so it asks.

The target belongs to the correction, and each says what it does against neutral. **Neutral, in-ear (diffuse field)** and **Neutral, over-ear (diffuse field)**, offered first for each, have no bass or treble preference; Harman's targets are neutral plus the bass and treble most listeners preferred. Moving an AutoEQ correction from Harman to neutral takes Harman's preference out, which leaves room for a tuning of your own on top.

## One correction at a time

An EQ that plays others may hold only one correction, and a correction that already includes a tuning counts as one. Two corrections would each try to undo the same device, so the colour would be taken out twice and the sound would end up worse than with neither. kōan refuses a second one and says which correction it already has. Add your extra changes as a tuning instead.

An EQ made before kōan refused this still plays, and its page says what's wrong: "This EQ corrects twice: Cantor (AutoEQ) and Performer 8S (includes a tuning). Keep one."

The top of each EQ's page says what the chain does, one line per role, each with its badge: "Correction: AFUL Performer 8S → Harman in-ear 2019 (from measurement)", then "Tuning: Warm bass". A correction with a tuning in it reads "AFUL Performer 8S Lush (correction + tuning in one)".

## Device, target, tuning

Each output's EQ is three choices, made on the EQ page as the chain the music goes through, and it reads as a sentence: "Music to Topping E30, corrected by AFUL Performer 8S to Neutral, in-ear (diffuse field), then tuned with Lush".

- **Correction** is the correction for what the output plays through: headphones or speakers. Choosing it is all most people need.
- **Target** belongs to the correction: what it makes neutral mean. Harman's targets add the bass and treble most listeners prefer; neutral (diffuse field) adds nothing.
- **Tuning** is optional, behind **Add EQ**: your taste on top, one or more EQs in order.

kōan builds the chain itself, so no EQ that plays others has to be made by hand: the correction, then the tuning. A tuning can say which target it was made against, on its own page under **Made against**. On headphones corrected to another target, kōan plays the difference between the two first, so the tuning sounds as it was made to whatever corrects the headphones. A finished preset like Qudelix's "Lush" fixes that difference into one EQ for one pair of headphones; kōan works it out for each pair as it plays. A correction built from a measurement is instead fitted again to the tuning's target, unless that would leave out an EQ or add a note to the chain, in which case the difference is played as above and the EQ page shows that target difference. With a tuning on, the level is set from what the correction and tuning play together: a preamp set by hand on the correction applies only when it plays alone. A tuning whose target isn't known plays as it is, and kōan offers the target its curve looks made against for you to accept.

A correction that already includes a tuning needs none on top; the EQ page says so, and offers to split it. A group of tunings works as a quick switch between them. The tuning is this device's choice for that output, as the correction is, and the menu by the output in the transport offers it too.

**Split into Correction + Tuning…**, on such a correction's page or the EQ page, takes it apart with a measurement of the device and the target it counts as neutral (Harman, usually). The correction is the target minus the measurement; the tuning is everything the correction does beyond it, saved as an EQ of its own, made against that target. A squig.link preset holds back the treble, where measurements disagree, so its tuning is taken against the fitted correction rather than the whole difference; otherwise the tuning would carry this measurement's treble peaks, inverted, to every other device. A preview draws the two and their sum against the original before anything is saved. The outputs that played it then play the correction with the tuning on top, and the tuning works on any other device too. The original is kept.

## Which way for your headphones

1. **Search for your headphones** in **Find in AutoEQ…** under EQ in Settings. If they are there, install them. You have a correction, and you can switch its target on the correction's page.
2. **If they are not in AutoEQ**, choose **Find a Measurement…** and search for them under **Find it on squig.link**. kōan searches the catalogues of the reviewers' squig.link sites, each kept for a day, and lists every measurement of that model with its site, its rig where the site says, and its variant: tips, inserts, a port. Pick one measured on the rig your target assumes, and to match a site's own EQ presets, the measurement from that site: two sites' measurements of one model differ by a decibel or so, and the correction follows the one you pick. kōan fetches its left and right channels, averages them, subtracts the calibration the site applies to every measurement it shows (squig.link's own uses its IEF 2023 calibration), and credits the site on the correction. graph.hangout.audio (Crinacle's 711, 5128 and headphone databases) serves its measurements only to its own pages, so its results are listed last, greyed, and open the site in a browser instead. A measurement exported from squig.link or REW as a CSV file works too, taken as it is: squig.link's export is already averaged and calibrated, as the site shows it, while a site's raw left or right file is neither, so a correction from one will not match that site's presets. Then pick in-ear or over-ear, and a target.

   A speaker measured on a Klippel Near-Field Scanner, as Audio Science Review publishes them, is read from its `SPL Horizontal.txt` and `SPL Vertical.txt` exports. Given both (choose the two files together; on the Mac, choosing one brings the other from beside it), the correction is made from the speaker's listening window as CTA-2034 defines it: on-axis, ±10° vertical and ±10°, ±20° and ±30° horizontal, averaged in power, which predicts how a speaker sounds at the listening position better than its on-axis response alone. Given one, the on-axis response. The importer says which it used. A speaker is corrected to Flat, 0 dB across the band: a flat listening window is what CTA-2034 and spinorama measurements aim for, and headphone targets, which describe sound at the eardrum, are not offered for one (nor Flat for headphones). For a room tilt, add it as a tuning. Targets you have added yourself are offered for speakers too, since one may be a room curve. The fit is the one headphones get, so it leaves a speaker's treble above about 6 kHz alone: there the response at the listening position is set more by the speaker's directivity and the room than by anything a listening window can correct.

   A file the importer cannot read is refused on the spot, saying what it expected and what it found in the file.
3. **If you already have an EQ** made for your headphones or speakers, import it and answer "What is this EQ?". Choose *Correction* if it only corrects them, *Correction + Tuning* if it's named for a sound or adds bass of its own, or *Tuning* if it's taste for on top. *Decide Later* leaves it a tuning, which you can change on its page. Imported from a stage's *Add…* on the EQ page, it goes straight into that device's chain: a correction as its correction, a tuning on the end of its tuning. For a neutral correction, say which target it was made for if you know; if you don't, choose Unknown, and target switching stays off.

Then add any tunings you like on top. See [Equalisation and convolution](dsp.md) for everything else EQs can do.
