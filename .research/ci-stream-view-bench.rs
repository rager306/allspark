use pa_tui::{chat::{ChatEntry,AssistantMessage,MessageBlock},view::AgentView,theme::{Theme,ColorMode}};
fn main(){
 let mut v=AgentView::new(Theme::builtin("prime",ColorMode::TrueColor));
 v.push_entry(ChatEntry::Assistant(Box::new(AssistantMessage{blocks:vec![MessageBlock::Text("start".into())],has_tool_calls:false,streaming:true,error:None,aborted:false})));
 v.render_frame(100,30);
 let text="history ".repeat(6000);let(mut mutation,mut paint)=(std::time::Duration::ZERO,std::time::Duration::ZERO);
 for len in (480..=text.len()).step_by(480){
 let t=std::time::Instant::now();v.prepare_entry_mutation(0);
 if let ChatEntry::Assistant(m)=&mut v.chat[0]{m.blocks=vec![MessageBlock::Text(text[..len].into())];}v.mark_entry_stale(0);mutation+=t.elapsed();
 let t=std::time::Instant::now();std::hint::black_box(v.render_frame(100,30));paint+=t.elapsed();
 }
 println!("100 growing48KB prefixes: mutation={mutation:?}; frame={paint:?}");
}
