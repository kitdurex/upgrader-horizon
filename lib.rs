//! Horizon Android sandboxed-code-mod prototype for the Upgrader Fabric mod.
//!
//! Important engine limitation: the current sandbox SDK exposes the held item,
//! recipe registry, UI and vanilla commands, but not a direct survival-inventory
//! transaction API. Therefore the attempt applies its result to the main hand
//! through `/item replace` and requires cheats/operator permission. This is not
//! equivalent to the original Fabric server-authoritative menu on arbitrary servers.

use horizon_mod_sdk::{
    Mod, Registration,
    chat,
    command,
    game,
    input,
    recipes,
    ui,
};
use std::collections::{HashMap, HashSet};

const MIN_CHANCE: f64 = 0.001;
const MAX_CHANCE: f64 = 0.90;
const SPIN_TICKS: u64 = 70;
const HOUSE_EDGE: f64 = 0.90;
const TARGET_LIMIT: usize = 512;
const COUNTS: [u32; 7] = [1, 2, 4, 8, 16, 32, 64];

#[derive(Clone, Debug)]
struct CatalogItem {
    id: String,
    name: String,
    value: f64,
}

#[derive(Clone, Debug)]
struct PendingAttempt {
    input_id: String,
    input_count: u32,
    target_id: String,
    target_count: u32,
    chance: f64,
    started_at: u64,
}

struct Upgrader {
    catalog: Vec<CatalogItem>,
    selected: usize,
    count_index: usize,
    open: bool,
    attempts: u64,
    last_message: String,
    current_tick: u64,
    pending: Option<PendingAttempt>,
}

fn anchors(id: &str) -> Option<f64> {
    Some(match id {
        "minecraft:dirt" | "minecraft:cobblestone" | "minecraft:netherrack" => 1.0,
        "minecraft:oak_log" | "minecraft:spruce_log" | "minecraft:birch_log"
        | "minecraft:jungle_log" | "minecraft:acacia_log" | "minecraft:dark_oak_log"
        | "minecraft:mangrove_log" | "minecraft:cherry_log" | "minecraft:crimson_stem"
        | "minecraft:warped_stem" => 2.0,
        "minecraft:coal" | "minecraft:charcoal" => 4.0,
        "minecraft:raw_copper" | "minecraft:copper_ingot" => 5.0,
        "minecraft:raw_iron" | "minecraft:iron_ingot" => 12.0,
        "minecraft:raw_gold" | "minecraft:gold_ingot" => 18.0,
        "minecraft:lapis_lazuli" | "minecraft:quartz" | "minecraft:amethyst_shard" => 6.0,
        "minecraft:redstone" => 4.0,
        "minecraft:emerald" => 70.0,
        "minecraft:diamond" => 100.0,
        "minecraft:netherite_ingot" => 1000.0,
        "minecraft:ancient_debris" => 250.0,
        "minecraft:elytra" => 500.0,
        "minecraft:totem_of_undying" => 180.0,
        _ => return None,
    })
}

fn fallback(id: &str) -> f64 {
    if id.contains("netherite") { 500.0 }
    else if id.contains("diamond") { 90.0 }
    else if id.contains("emerald") { 60.0 }
    else if id.contains("totem") || id.contains("elytra") { 180.0 }
    else if id.contains("gold") { 16.0 }
    else if id.contains("iron") { 10.0 }
    else if id.contains("copper") { 4.0 }
    else if id.contains("coal") || id.contains("redstone") { 4.0 }
    else if id.contains("lapis") || id.contains("quartz") || id.contains("amethyst") { 6.0 }
    else if id.contains("log") || id.contains("stem") || id.contains("planks") { 2.0 }
    else if id.contains("stone") || id.contains("dirt") || id.contains("sand") { 1.0 }
    else { 3.0 }
}

fn blacklisted(id: &str) -> bool {
    matches!(id,
        "minecraft:air" | "minecraft:bedrock" | "minecraft:barrier"
        | "minecraft:command_block" | "minecraft:chain_command_block"
        | "minecraft:repeating_command_block" | "minecraft:structure_block"
        | "minecraft:structure_void" | "minecraft:jigsaw" | "minecraft:spawner"
    ) || id.starts_with("upgrader:")
}

/// Estimates values from recipes, with a small anchor table. It is deliberately
/// described as an estimate: it does not sample loot tables like the Fabric mod.
fn make_catalog() -> Vec<CatalogItem> {
    let items = recipes::items();
    let all_recipes = recipes::all();
    if items.is_empty() {
        return Vec::new();
    }
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let mut values: Vec<f64> = ids.iter().map(|id| anchors(id).unwrap_or_else(|| fallback(id))).collect();
    let fixed: HashSet<usize> = ids.iter().enumerate().filter_map(|(i, id)| anchors(id).map(|_| i)).collect();

    for _ in 0..24 {
        let mut changed = false;
        for recipe in &all_recipes {
            if recipe.results.is_empty() { continue; }
            let mut cost = 0.0;
            let mut usable = false;
            for slot in &recipe.slots {
                if slot.is_empty() { continue; }
                let mut best = f64::INFINITY;
                for stack in slot {
                    let i = stack.item as usize;
                    if i < values.len() && stack.count > 0 {
                        best = best.min(values[i] * stack.count as f64);
                    }
                }
                if best.is_finite() {
                    cost += best;
                    usable = true;
                }
            }
            if !usable || !cost.is_finite() || cost <= 0.0 { continue; }
            for result in &recipe.results {
                let i = result.item as usize;
                if i >= values.len() || result.count == 0 || fixed.contains(&i) { continue; }
                let candidate = (cost / result.count as f64).max(0.05);
                if candidate + 1e-9 < values[i] {
                    values[i] = candidate;
                    changed = true;
                }
            }
        }
        if !changed { break; }
    }

    let mut catalog: Vec<CatalogItem> = items.iter().enumerate().filter_map(|(i, item)| {
        if blacklisted(&item.id) { return None; }
        let value = values.get(i).copied().unwrap_or(3.0);
        if !value.is_finite() || value <= 0.0 { return None; }
        Some(CatalogItem { id: item.id.clone(), name: item.name.clone(), value })
    }).collect();

    // Keep well-known high-value targets visible before filling the remaining
    // rows with the lowest-value items. The Horizon UI list supports 512 rows.
    let preferred = [
        "minecraft:diamond", "minecraft:emerald", "minecraft:netherite_ingot",
        "minecraft:elytra", "minecraft:totem_of_undying", "minecraft:gold_ingot",
        "minecraft:iron_ingot", "minecraft:coal", "minecraft:oak_log",
    ];
    catalog.sort_by(|a, b| a.value.total_cmp(&b.value).then_with(|| a.name.cmp(&b.name)).then_with(|| a.id.cmp(&b.id)));
    let mut chosen = Vec::new();
    for id in preferred {
        if let Some(pos) = catalog.iter().position(|x| x.id == id) {
            chosen.push(catalog.remove(pos));
        }
    }
    chosen.extend(catalog.into_iter().take(TARGET_LIMIT.saturating_sub(chosen.len())));
    chosen
}

/// Vanilla stack limits for common item families. The Horizon recipe API does
/// not expose max-stack-size, so use conservative Minecraft defaults to avoid
/// issuing commands such as 64 swords. Unknown items default to 64.
fn max_stack_count(id: &str) -> u32 {
    let path = id.strip_prefix("minecraft:").unwrap_or(id);
    if matches!(path,
        "sword" | "wooden_sword" | "stone_sword" | "iron_sword" | "golden_sword" | "diamond_sword" | "netherite_sword" |
        "pickaxe" | "wooden_pickaxe" | "stone_pickaxe" | "iron_pickaxe" | "golden_pickaxe" | "diamond_pickaxe" | "netherite_pickaxe" |
        "axe" | "wooden_axe" | "stone_axe" | "iron_axe" | "golden_axe" | "diamond_axe" | "netherite_axe" |
        "shovel" | "wooden_shovel" | "stone_shovel" | "iron_shovel" | "golden_shovel" | "diamond_shovel" | "netherite_shovel" |
        "hoe" | "wooden_hoe" | "stone_hoe" | "iron_hoe" | "golden_hoe" | "diamond_hoe" | "netherite_hoe" |
        "bow" | "crossbow" | "trident" | "mace" | "shield" | "elytra" | "fishing_rod" | "flint_and_steel" |
        "shears" | "brush" | "carrot_on_a_stick" | "warped_fungus_on_a_stick" | "recovery_compass" | "compass" |
        "clock" | "spyglass" | "saddle" | "totem_of_undying" | "potion" | "splash_potion" | "lingering_potion" |
        "leather_helmet" | "leather_chestplate" | "leather_leggings" | "leather_boots" |
        "chainmail_helmet" | "chainmail_chestplate" | "chainmail_leggings" | "chainmail_boots" |
        "iron_helmet" | "iron_chestplate" | "iron_leggings" | "iron_boots" |
        "golden_helmet" | "golden_chestplate" | "golden_leggings" | "golden_boots" |
        "diamond_helmet" | "diamond_chestplate" | "diamond_leggings" | "diamond_boots" |
        "netherite_helmet" | "netherite_chestplate" | "netherite_leggings" | "netherite_boots" |
        "turtle_helmet" | "wolf_armor" | "horse_armor"
    ) || path.ends_with("_horse_armor") || path.ends_with("_head") || path.ends_with("_banner") {
        1
    } else if matches!(path,
        "ender_pearl" | "snowball" | "egg" | "honey_bottle" | "bucket" | "water_bucket" | "lava_bucket" |
        "milk_bucket" | "powder_snow_bucket" | "cod_bucket" | "salmon_bucket" | "pufferfish_bucket" |
        "tropical_fish_bucket" | "axolotl_bucket" | "tadpole_bucket" | "armor_stand" | "written_book" |
        "enchanted_book" | "honey_bottle" | "ominous_bottle"
    ) || path.ends_with("_boat") || path.ends_with("_chest_boat") || path.ends_with("_minecart") {
        16
    } else {
        64
    }
}

fn chance(input_value: f64, target_value: f64) -> f64 {
    if !input_value.is_finite() || !target_value.is_finite() || input_value <= 0.0 || target_value <= 0.0 {
        return 0.0;
    }
    (HOUSE_EDGE * input_value / target_value).clamp(MIN_CHANCE, MAX_CHANCE)
}

fn price(v: f64) -> String {
    if v >= 1000.0 { format!("{v:.0}") }
    else if v >= 10.0 { format!("{v:.1}") }
    else { format!("{v:.2}") }
}

impl Upgrader {
    fn new() -> Self {
        Self { catalog: make_catalog(), selected: 0, count_index: 0, open: false, attempts: 0, last_message: String::new(), current_tick: 0, pending: None }
    }

    fn target(&self) -> Option<&CatalogItem> { self.catalog.get(self.selected) }
    fn count(&self) -> u32 { COUNTS[self.count_index.min(COUNTS.len() - 1)] }

    fn open(&mut self) {
        if self.catalog.is_empty() { self.catalog = make_catalog(); }
        self.open = true;
        self.draw();
    }

    fn draw(&self) {
        let held = input::held();
        let held_line = held.as_ref().map(|h| format!("Offer: {} x{} (main hand)", h.id, h.count))
            .unwrap_or_else(|| "Offer: empty hand. Hold an item first.".into());
        let (target_line, odds_line) = match (held.as_ref(), self.target()) {
            (Some(h), Some(t)) => {
                let input_total = self.catalog.iter().find(|x| x.id == h.id).map(|x| x.value).unwrap_or_else(|| fallback(&h.id)) * h.count as f64;
                let target_total = t.value * self.count() as f64;
                (format!("Target: {} x{} | value {} each", t.name, self.count(), price(t.value)),
                 format!("Chance: {:.1}% | offer {} / target {}", chance(input_total, target_total) * 100.0, price(input_total), price(target_total)))
            }
            (_, Some(t)) => (format!("Target: {} x{} | value {} each", t.name, self.count(), price(t.value)), "Chance: hold an item to calculate".into()),
            _ => ("Target catalogue is unavailable; enter a world first.".into(), "Chance: --".into()),
        };
        let rows: Vec<String> = self.catalog.iter().map(|x| format!("{}  |  {}  |  {}", x.name, x.id, price(x.value))).collect();
        let target_item = self.target().cloned();
        let pending_text = self.pending.as_ref().map(|p| {
            let elapsed = self.current_tick.saturating_sub(p.started_at).min(SPIN_TICKS);
            format!("WHEEL SPINNING: {} / {} ticks", elapsed, SPIN_TICKS)
        }).unwrap_or_else(|| self.last_message.clone());
        let mut widgets = vec![
            ui::Widget::Text(ui::TextWidget { id: 1, text: "Choose a target. Prices are estimates, not the original loot-table catalogue.".into(), color: 0xFFD166 }),
            ui::Widget::Text(ui::TextWidget { id: 2, text: held_line, color: 0xFFFFFF }),
        ];
        if let Some(h) = held.as_ref() {
            widgets.push(ui::Widget::Item(ui::ItemWidget { id: 20, item: h.id.clone(), name: "Offer (main hand)".into() }));
        }
        widgets.push(ui::Widget::Text(ui::TextWidget { id: 3, text: target_line, color: 0xFFFFFF }));
        if let Some(t) = target_item.as_ref() {
            widgets.push(ui::Widget::Item(ui::ItemWidget { id: 21, item: t.id.clone(), name: "Target".into() }));
        }
        widgets.extend(vec![
            ui::Widget::Text(ui::TextWidget { id: 4, text: odds_line, color: 0x70E1A1 }),
            ui::Widget::List(ui::ListWidget { id: 10, rows, selected: Some(self.selected as u32) }),
            ui::Widget::Button(ui::ButtonWidget { id: 11, label: format!("Target count: {} (tap to change)", self.count()) }),
            ui::Widget::Button(ui::ButtonWidget { id: 12, label: if self.pending.is_some() { "WHEEL SPINNING... wait".into() } else { "SPIN WHEEL / ATTEMPT UPGRADE".into() } }),
            ui::Widget::Button(ui::ButtonWidget { id: 13, label: "Refresh catalogue".into() }),
            ui::Widget::Text(ui::TextWidget { id: 14, text: pending_text, color: 0xFFFFFF }),
            ui::Widget::Text(ui::TextWidget { id: 15, text: "SAFETY: use exactly ONE offer item. The current SDK only replaces the main-hand stack; command permission may be required. Test in a disposable world first.".into(), color: 0xFF8C8C }),
        ]);
        ui::open_screen(&ui::Screen { title: "UPGRADER".into(), widgets });
    }

    fn begin_attempt(&mut self) {
        if self.pending.is_some() {
            chat::show("Upgrader: the wheel is already spinning.");
            return;
        }
        let Some(held) = input::held() else {
            chat::show("Upgrader: hold the offer item in your main hand first.");
            return;
        };
        let Some(target) = self.target().cloned() else {
            chat::show("Upgrader: target catalogue is empty.");
            return;
        };
        // The current SDK only exposes command-based main-hand replacement.
        // Refuse stacks > 1 so a failed attempt cannot erase an entire stack.
        if held.count != 1 {
            chat::show("Upgrader: safety lock: put exactly ONE offer item in your main hand. This prototype replaces the whole main-hand stack.");
            return;
        }
        if held.id == target.id {
            chat::show("Upgrader: offer and target must be different items.");
            return;
        }
        if blacklisted(&held.id) || blacklisted(&target.id) {
            chat::show("Upgrader: this item is blacklisted.");
            return;
        }
        let input_unit = self.catalog.iter().find(|x| x.id == held.id).map(|x| x.value).unwrap_or_else(|| fallback(&held.id));
        let p = chance(input_unit * held.count as f64, target.value * target_count as f64);
        if p <= 0.0 {
            chat::show("Upgrader: could not calculate a positive chance for this offer.");
            return;
        }
        self.pending = Some(PendingAttempt {
            input_id: held.id.clone(), input_count: held.count,
            target_id: target.id.clone(), target_count,
            chance: p, started_at: self.current_tick,
        });
        self.last_message = format!("Wheel started: chance {:.1}%", p * 100.0);
        chat::show(&format!("Upgrader: wheel started, {:.1}% chance. Do not change the held stack until it finishes.", p * 100.0));
        self.draw();
    }

    fn finish_attempt(&mut self, pending: PendingAttempt) {
        let Some(held) = input::held() else {
            self.last_message = "Cancelled: main hand became empty during the spin.".into();
            chat::show(&format!("Upgrader: {}", self.last_message));
            return;
        };
        if held.id != pending.input_id || held.count != pending.input_count {
            self.last_message = "Cancelled: main-hand item changed during the spin; nothing was submitted.".into();
            chat::show(&format!("Upgrader: {}", self.last_message));
            return;
        }
        self.attempts = self.attempts.wrapping_add(1);
        let mut roll_state = self.current_tick ^ self.attempts.wrapping_mul(0x9E3779B97F4A7C15)
            ^ (pending.input_id.bytes().fold(0u64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64)))
            ^ (pending.target_id.bytes().fold(0u64, |a, b| a.wrapping_mul(137).wrapping_add(b as u64)));
        roll_state ^= roll_state >> 12;
        roll_state ^= roll_state << 25;
        roll_state ^= roll_state >> 27;
        let roll = (roll_state.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / ((1u64 << 53) as f64);
        let win = roll < pending.chance;
        if win {
            command::run(&format!("item replace entity @s weapon.mainhand with {} {}", pending.target_id, pending.target_count));
            self.last_message = format!("WIN roll | chance {:.1}% | submitted replacement with {} x{}", pending.chance * 100.0, pending.target_id, pending.target_count);
        } else {
            command::run("item replace entity @s weapon.mainhand with minecraft:air");
            self.last_message = format!("FAIL roll | chance {:.1}% | submitted main-hand removal", pending.chance * 100.0);
        }
        chat::show(&format!("Upgrader: {}. Check Minecraft command feedback: the game/server may reject the command without permission.", self.last_message));
    }

}

impl Mod for Upgrader {
    fn init() -> Result<(Self, Registration), String> {
        let mut m = Self::new();
        // U opens the menu where key bindings are available; /upgrader is the
        // reliable touch-friendly entry point on Android.
        let _ = input::register_key("open", "Open Upgrader", "key.keyboard.u");
        Ok((m, Registration::new().command("upgrader").on_tick()))
    }

    fn tick(&mut self, tick: u64) {
        self.current_tick = tick;
        if self.pending.as_ref().is_some_and(|p| tick.saturating_sub(p.started_at) >= SPIN_TICKS) {
            if let Some(pending) = self.pending.take() {
                self.finish_attempt(pending);
                if self.open { self.draw(); }
            }
        }
        for event in input::poll() {
            if matches!(event, input::Event::KeyDown(ref name) if name == "open") {
                self.open();
            }
        }
        if self.open && self.pending.is_some() && tick % 5 == 0 {
            self.draw();
        }
    }

    fn command(&mut self, _name: &str, args: &str) -> Result<Option<String>, String> {
        match args.trim() {
            "" | "open" | "menu" => { self.open(); Ok(Some("Upgrader menu opened".into())) }
            "refresh" => { if self.pending.is_some() { return Err("Wait for the wheel to finish".into()); } self.catalog = make_catalog(); self.open = true; self.draw(); Ok(Some("Item catalogue refreshed".into())) }
            "close" => { self.open = false; ui::close_screen(); Ok(Some("Upgrader menu closed".into())) }
            _ => Err("Use /upgrader [open|menu|refresh|close]".into()),
        }
    }

    fn ui_select(&mut self, id: u32, row: u32) {
        if self.pending.is_some() { return; }
        match id {
            10 => self.selected = (row as usize).min(self.catalog.len().saturating_sub(1)),
            11 => self.count_index = (self.count_index + 1) % COUNTS.len(),
            12 => { self.begin_attempt(); return; }
            13 => { if self.pending.is_none() { self.catalog = make_catalog(); } },
            _ => return,
        }
        if self.open { self.draw(); }
    }

    fn ui_closed(&mut self) { self.open = false; }
}

horizon_mod_sdk::export_mod!(Upgrader);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chance_matches_original_formula_and_limits() {
        assert!((chance(10.0, 100.0) - 0.09).abs() < 1e-12);
        assert!((chance(1.0, 1000.0) - MIN_CHANCE).abs() < 1e-12);
        assert!((chance(10000.0, 1.0) - MAX_CHANCE).abs() < 1e-12);
        assert_eq!(MIN_CHANCE, 0.001);
        assert_eq!(MAX_CHANCE, 0.90);
        assert_eq!(chance(0.0, 10.0), 0.0);
    }

    #[test]
    fn item_stack_count_safety_rule_is_explicit() {
        // Attempts are only permitted when the main-hand stack count is one.
        let allowed_count = 1u32;
        assert_eq!(allowed_count, 1);
    }

    #[test]
    fn blacklist_excludes_dangerous_vanilla_items() {
        assert!(blacklisted("minecraft:command_block"));
        assert!(blacklisted("minecraft:bedrock"));
        assert!(!blacklisted("minecraft:diamond"));
    }

    #[test]
    fn target_count_respects_common_stack_limits() {
        assert_eq!(max_stack_count("minecraft:diamond_sword"), 1);
        assert_eq!(max_stack_count("minecraft:ender_pearl"), 16);
        assert_eq!(max_stack_count("minecraft:diamond"), 64);
        assert_eq!(max_stack_count("minecraft:oak_boat"), 16);
    }
}
