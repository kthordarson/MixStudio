//! Processed transition audition: temporary snapshot filtering, never project edits.
use super::*;

pub(super) struct Audition {
    incoming: usize,
    side: u8,
    from: f64,
    to: f64,
    repeat: bool,
    stamp: Vec<u64>,
    restore_loop: Option<(f64, f64)>,
    restore_loop_on: bool,
}
impl App {
    pub(super) fn arrangement_locked(&self) -> bool {
        self.lock || self.mix.has_transition_locks()
    }
    pub(super) fn set_transition_protection(&mut self, i: usize, locked: bool) {
        if self
            .mix
            .tracks
            .get(i)
            .is_none_or(|t| t.trans.locked == locked)
        {
            return;
        }
        self.snapshot();
        self.mix.tracks[i].trans.locked = locked;
        self.mix.sync_transition(i);
        self.drag = None;
        self.tl_drag = None;
        self.grid_drag = None;
        self.bm_drag = None;
        self.tc_drag = None;
        self.status=if locked {"Transition protected. Unlock protected transitions to move, cut or reorder the arrangement."}else{"Transition unlocked. Undo: Ctrl+Z."}.into();
    }
    pub(super) fn transition_source_ranges(
        &self,
        incoming: usize,
        side: usize,
    ) -> Vec<(f64, f64, f64, f64)> {
        let Some((_, _, s, e)) = self
            .mix
            .logical_transitions()
            .into_iter()
            .find(|x| x.1 == incoming)
        else {
            return vec![];
        };
        let Some(members) = self.transition_members(incoming) else {
            return vec![];
        };
        let mut ranges = vec![];
        for &i in &members[side.min(1)] {
            let t = &self.mix.tracks[i];
            let g = t.grid();
            let lo = s.max(t.vis_start(0.0));
            let hi = e.min(t.end(0.0));
            if hi <= lo {
                continue;
            }
            let mut points = vec![lo];
            points.extend(g.cuts().filter(|b| *b > lo && *b < hi));
            points.push(hi);
            for w in points.windows(2) {
                let at = (w[0] + w[1]) * 0.5;
                ranges.push((w[0], w[1], g.region_sec(w[0], at), g.region_sec(w[1], at)));
            }
        }
        ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
        ranges
    }
    pub(super) fn start_transition_audition(&mut self, i: usize, side: u8, repeat: bool) {
        let Some((_, _, s, e)) = self
            .mix
            .logical_transitions()
            .into_iter()
            .find(|x| x.1 == i)
        else {
            return;
        };
        let Some(stamp) = self.alignment_stamp(i) else {
            self.status = "Load both source tracks before listening.".into();
            return;
        };
        if self.transition_members(i).is_none_or(|g| {
            g.iter()
                .flatten()
                .any(|k| !self.store.contains_key(&self.mix.tracks[*k].path))
        }) {
            self.status = "Load every source fragment before listening.".into();
            return;
        }
        self.stop_listen();
        self.stop_comparison();
        self.stop_transition_audition();
        let from = (s - self.transition_pre as f64 * 4.0).max(0.0);
        let to = (e + self.transition_post as f64 * 4.0).min(self.mix.length_beats());
        if to <= from {
            return;
        }
        self.transition_audition = Some(Audition {
            incoming: i,
            side: side.clamp(1, 3),
            from,
            to,
            repeat,
            stamp,
            restore_loop: self.active_loop,
            restore_loop_on: self.loop_on,
        });
        self.loop_on = repeat;
        self.set_loop(if repeat { Some((from, to)) } else { None });
        let _ = self.out.snapshot(Arc::new(self.build_playback_snap()));
        self.seek(from);
        let _ = self.out.tx.send(Cmd::Play(true));
    }
    pub(super) fn switch_transition_side(&mut self, side: u8) {
        if let Some(a) = &mut self.transition_audition {
            a.side = side.clamp(1, 3);
            let _ = self.out.snapshot(Arc::new(self.build_playback_snap()));
        }
    }
    pub(super) fn stop_transition_audition(&mut self) {
        if let Some(a) = self.transition_audition.take() {
            let _ = self.out.tx.send(Cmd::Play(false));
            self.loop_on = a.restore_loop_on;
            self.set_loop(a.restore_loop);
            let _ = self.out.snapshot(Arc::new(self.build_snap()));
        }
    }
    pub(super) fn validate_transition_audition(&mut self) {
        if let Some(a) = &self.transition_audition {
            if self.tab != Tab::Transition
                || self.sel_in != Some(a.incoming)
                || self.alignment_stamp(a.incoming).as_ref() != Some(&a.stamp)
                || (!a.repeat && self.beat() >= a.to)
                || self.beat() < a.from - 0.001
            {
                self.stop_transition_audition();
            }
        }
    }
    pub(super) fn filter_transition_audition(&self, snap: &mut Snap) {
        let Some(a) = &self.transition_audition else {
            return;
        };
        if self.tab != Tab::Transition
            || self.alignment_stamp(a.incoming).as_ref() != Some(&a.stamp)
        {
            return;
        }
        let Some(members) = self.transition_members(a.incoming) else {
            return;
        };
        let mut loaded = 0;
        for (i, t) in self.mix.tracks.iter().enumerate() {
            if !self.store.contains_key(&t.path) {
                continue;
            }
            if !members.iter().flatten().any(|k| *k == i) {
                snap.tracks[loaded].vis = snap.tracks[loaded].end;
            }
            loaded += 1;
        }
        // Keep both sides and all transition/stem/FX routing. Deck mute isolates
        // one side after its transition processing without renumbering voices.
        for d in 0..4 {
            let allowed = members.iter().enumerate().any(|(side, group)| {
                (a.side == 3 || a.side == side as u8 + 1)
                    && group.iter().any(|i| self.mix.tracks[*i].lane == d)
            });
            snap.decks[d].mute = !allowed;
            snap.decks[d].solo = false;
        }
    }
    pub(super) fn transition_tab(&mut self, ui: &mut egui::Ui) {
        // Wide windows: one header row with the primary actions, then three weighted
        // columns (Align | Listen & key | Shape), each clipped and scrolling on its own.
        if ui.available_width() >= 900.0 {
            let Some((o,i,s,e))=self.transition_header(ui) else { return; };
            ui.add_space(4.0);
            let area=ui.available_rect_before_wrap();
            let gap=18.0;
            let weights=[0.30,0.28,0.42];
            let w=area.width()-gap*(weights.len()-1) as f32;
            let mut x=area.left();
            for (k,wt) in weights.iter().enumerate() {
                let r=Rect::from_min_size(pos2(x,area.top()),vec2(w*wt,area.height()));
                if k>0 {ui.painter().vline(x-gap*0.5,area.y_range(),Stroke::new(1.0_f32,theme::line()));}
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(r),|ui| {
                    ui.set_clip_rect(r.intersect(ui.clip_rect()));
                    egui::ScrollArea::vertical().id_salt(("transition_col",k)).auto_shrink(false).show(ui,|ui| match k {
                        0=>self.transition_align_ui(ui,i),
                        1=>{self.transition_listen_ui(ui,i);self.transition_details_ui(ui,o,i);}
                        _=>self.transition_shape_ui(ui,o,i,s,e),
                    });
                });
                x+=w*wt+gap;
            }
            ui.allocate_rect(area,Sense::hover());
            return;
        }
        egui::ScrollArea::vertical().id_salt("transition_tools").show(ui,|ui| {
            let Some((o,i,s,e))=self.transition_header(ui) else { return; };
            self.transition_align_ui(ui,i);
            self.transition_listen_ui(ui,i);
            self.transition_details_ui(ui,o,i);
            self.transition_shape_ui(ui,o,i,s,e);
        });
    }
    /// One wrapped header row: picker, A → B, overlap size, protection and play/stop.
    /// `None` when the mix has no transition yet.
    fn transition_header(&mut self, ui: &mut egui::Ui) -> Option<(usize,usize,f64,f64)> {
        let transitions=self.mix.logical_transitions();
        if transitions.is_empty() {
            ui.label("No transition yet. Add two overlapping tracks.");
            if self.mix.has_transition_locks() && ui.button("Unlock all transitions").clicked() {self.snapshot();for t in &mut self.mix.tracks {t.trans.locked=false;}}
            return None;
        }
        let mut choice=self.sel_in.and_then(|i|transitions.iter().position(|x|x.1==i)).unwrap_or(0);
        let old=choice;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Transition");
            egui::ComboBox::from_id_salt("transition_choice").selected_text(format!("{} of {}",choice+1,transitions.len())).show_ui(ui,|ui| {for (k,(o,i,_,_)) in transitions.iter().enumerate() {ui.selectable_value(&mut choice,k,format!("{}: {} to {}",k+1,self.mix.tracks[*o].title,self.mix.tracks[*i].title));}});
            if ui.add_enabled(choice>0,egui::Button::new("◀")).on_hover_text("Previous transition").clicked() {choice-=1;}
            if ui.add_enabled(choice+1<transitions.len(),egui::Button::new("▶")).on_hover_text("Next transition").clicked() {choice+=1;}
        });
        if old!=choice {self.stop_transition_audition();}
        let (o,i,s,e)=transitions[choice];
        if self.transition_audition.as_ref().is_some_and(|a|a.incoming!=i) {self.stop_transition_audition();}
        self.sel_in=Some(i);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("A: {}",self.mix.tracks[o].title)).strong());
            ui.label(egui::RichText::new("➡").color(theme::accent()));
            ui.label(egui::RichText::new(format!("B: {}",self.mix.tracks[i].title)).strong());
            ui.label(egui::RichText::new(format!("·  {:.2} bars · {:.1} s · mix bars {:.2}–{:.2}",(e-s)/4.0,self.mix.secs(s,e),s/4.0+1.0,e/4.0+1.0)).color(theme::dim()));
            ui.separator();
            let mut protected=self.mix.tracks[i].trans.locked;
            if ui.checkbox(&mut protected,"Protect this transition").on_hover_text("Save the protection with the project. Freeze arrangement placement/ratios and this transition’s settings until unlocked.").changed() {self.set_transition_protection(i,protected);}
            if self.mix.has_transition_locks() && ui.button("Unlock all").on_hover_text("Unlock every protected transition").clicked() {self.snapshot();for t in &mut self.mix.tracks {t.trans.locked=false;}}
            ui.separator();
            let playing=self.transition_audition.as_ref().is_some_and(|a|a.incoming==i);
            if playing {
                if ui.button("⏹ Stop audition").clicked() {self.stop_transition_audition();}
            } else if ui.add(egui::Button::new(egui::RichText::new("▶ Play transition").color(theme::bg())).fill(theme::accent())).on_hover_text("Processed audition of both sides, with the Listen before/after bars").clicked() {
                self.start_transition_audition(i,3,self.transition_repeat);
            }
        });
        if self.mix.has_transition_locks() {ui.colored_label(C::LIGHT_YELLOW,"Protected transitions freeze arrangement moves, cuts, reorder and automatic beat ratios. Unlock to change placement.");}
        Some((o,i,s,e))
    }
    fn transition_align_ui(&mut self, ui: &mut egui::Ui, i: usize) {
        section(ui,"Beatmatch");
        self.beatmatch_ui(ui,false);
        section(ui,"Source ranges");
        for (side,label) in [(0,"A outgoing"),(1,"B incoming")] {
            let ranges=self.transition_source_ranges(i,side);
            let line=ranges.iter().map(|(_,_,a,b)|format!("{} – {}",source_time(*a),source_time(*b))).collect::<Vec<_>>().join("; ");
            ui.label(egui::RichText::new(format!("{label} · source {line}")).color(theme::dim()));
        }
    }
    fn transition_details_ui(&mut self, ui: &mut egui::Ui, o: usize, i: usize) {
        section(ui,"Key & gain");
        self.harmony_ui(ui,o,i);
        ui.horizontal_wrapped(|ui| {self.song_gain_control(ui,o,"A gain");self.song_gain_control(ui,i,"B gain");});
    }
    fn transition_shape_ui(&mut self, ui: &mut egui::Ui, o: usize, i: usize, s: f64, e: f64) {
        section(ui,"Shape");
        let locked=self.arrangement_locked();
        egui::Grid::new("transition_shape_grid").num_columns(2).spacing(vec2(10.0,6.0)).show(ui,|ui| {
            ui.label("Overlap length");
            ui.horizontal_wrapped(|ui| {
                for bars in [4,8,16,32] {
                    let on=((e-s)-bars as f64*4.0).abs()<0.01;
                    if ui.add_enabled(!locked,egui::Button::new(format!("{bars}")).selected(on)).on_hover_text(format!("{bars} bars")).clicked() {self.snapshot();self.mix.shift_logical_transition(i,(e-s)-bars as f64*4.0,true);}
                }
                ui.label(egui::RichText::new("bars").color(theme::dim()));
            });
            ui.end_row();
            let mut ratio=self.mix.tracks[i].trans.ratio;
            ui.label("Incoming beat ratio").on_hover_text(format!("Current mapping: one B source beat spans {:.3} mix beats. Auto selects a ratio when the arrangement is unlocked.",self.mix.tracks[i].beat_mult));
            ui.add_enabled_ui(!locked,|ui| {egui::ComboBox::from_id_salt("beat_ratio").selected_text(crate::model::RATIOS[ratio as usize].0).show_ui(ui,|ui| {for (k,(name,_)) in crate::model::RATIOS.iter().enumerate() {ui.selectable_value(&mut ratio,k as u8,*name);}});});
            ui.end_row();
            if ratio!=self.mix.tracks[i].trans.ratio {self.snapshot();self.mix.tracks[i].trans.ratio=ratio;self.mix.sync_transition(i);self.mix.resolve_ratios();}
        });
        let editable=!self.lock && !self.mix.tracks[i].trans.locked;
        ui.add_enabled_ui(editable,|ui| {
            let mut preset=self.mix.tracks[i].trans.preset;
            let mut stems=self.mix.tracks[i].trans.stems;
            egui::Grid::new("transition_stem_grid").num_columns(4).spacing(vec2(10.0,6.0)).show(ui,|ui| {
                ui.label("Preset");
                egui::ComboBox::from_id_salt("transition_preset").width(110.0).selected_text(preset.name()).show_ui(ui,|ui| {for p in Preset::ALL {ui.selectable_value(&mut preset,p,p.name());}});
                ui.end_row();
                for k in 0..4 {
                    ui.label(STEM_N[k]);
                    egui::ComboBox::from_id_salt(("transition_stem",k)).width(110.0).selected_text(stems[k].name()).show_ui(ui,|ui| {for mode in StemMode::ALL {ui.selectable_value(&mut stems[k],mode,mode.name());}});
                    if k%2==1 {ui.end_row();}
                }
            });
            if preset!=self.mix.tracks[i].trans.preset {self.snapshot();let t=&mut self.mix.tracks[i].trans;t.preset=preset;t.stems=match preset {Preset::BassSwap=>[StemMode::Crossfade,StemMode::Swap,StemMode::Crossfade,StemMode::Crossfade],Preset::EchoOut=>[StemMode::Swap;4],_=>[StemMode::Crossfade;4]};}
            else if stems!=self.mix.tracks[i].trans.stems {self.snapshot();self.mix.tracks[i].trans.stems=stems;}
            self.trans_curves_ui(ui,i);
        });
        self.mix.sync_transition(i);
        let stems_ready=[o,i].iter().all(|i|self.stems.contains_key(&self.mix.tracks[*i].path));
        ui.label(egui::RichText::new(if stems_ready {"Stems ready: stem choices and preset processing are active."}else{"Stems unavailable: the preset volume curve is used. Separate both tracks to hear individual stem choices."}).color(theme::dim()).size(11.0));
        ui.horizontal_wrapped(|ui| {
            if !stems_ready && ui.button("Separate stems for both").clicked() {self.request_stems(o);self.request_stems(i);}
            if ui.button("Open Beatgrid ➡ source grid tools").clicked() {self.stop_transition_audition();self.sel=Some(i);self.tab=Tab::Grid;}
        });
    }
    fn transition_listen_ui(&mut self, ui: &mut egui::Ui, incoming: usize) {
        section(ui, "Listen");
        ui.horizontal_wrapped(|ui| {
            ui.label("Pre-roll");
            ui.add(egui::DragValue::new(&mut self.transition_pre).range(0..=8).suffix(" bars"));
            ui.label("Post-roll");
            ui.add(egui::DragValue::new(&mut self.transition_post).range(0..=8).suffix(" bars"));
            ui.checkbox(&mut self.transition_repeat, "Repeat");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Monitor");
            for (side,label) in [(1,"A"),(3,"Both"),(2,"B")] {
                let on=self.transition_audition.as_ref().is_some_and(|a|a.incoming==incoming && a.side==side);
                if ui.add(egui::Button::new(label).selected(on).min_size(vec2(44.0,0.0))).on_hover_text("Temporarily override deck mute/solo for audition. Keep volume, EQ, transitions, stems and effects; shared effect tails carry over when switching.").clicked() {if self.transition_audition.is_some() {self.switch_transition_side(side);}else{self.start_transition_audition(incoming,side,self.transition_repeat);}}
            }
        });
        let status=match &self.transition_audition {
            Some(a)=>format!("{} mix bars {:.2}–{:.2}. Pre/post-roll and Repeat apply from the next start.",if a.repeat {"Looping"}else{"Playing once"},a.from/4.0+1.0,a.to/4.0+1.0),
            None=>"Processed audition · shared FX tails continue across A/B changes".into(),
        };
        ui.label(egui::RichText::new(status).color(theme::dim()).size(11.0));
    }
}
fn source_time(s: f64) -> String {
    format!(
        "{}:{:06.3}",
        (s.max(0.0) / 60.0).floor() as u64,
        s.max(0.0) % 60.0
    )
}
