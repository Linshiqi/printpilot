//! 项目名下的产出:三个工作台(调研 / 图片 / 建模)各自往项目上挂了什么。
//! 阶段门清单(pp_common::gate)和项目中枢都从这里取数。

use std::collections::HashMap;

use pp_common::gate::{ProjectFacts, ProjectImage, ProjectOverview};
use rusqlite::params;

use crate::{Db, DbResult};

/// 项目中枢里最多摆几张图
const OVERVIEW_IMAGES: u32 = 12;

impl Db {
    /// 所有项目的产出计数,一次查完(看板上每张卡片都要用)。没有任何产出的项目也在里面。
    pub fn all_project_facts(&self) -> DbResult<HashMap<String, ProjectFacts>> {
        let conn = self.r();
        let mut out: HashMap<String, ProjectFacts> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, TRIM(hypothesis) != '' FROM projects WHERE deleted_at IS NULL")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?;
            for row in rows {
                let (id, has_hypothesis) = row?;
                out.insert(
                    id,
                    ProjectFacts {
                        has_hypothesis,
                        ..Default::default()
                    },
                );
            }
        }
        // (SQL, 怎么把一行写进 ProjectFacts);每条都是 `project_id, 数量[, 数量]`
        let mut fill = |sql: &str, apply: &dyn Fn(&mut ProjectFacts, u32, u32)| -> DbResult<()> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
            for row in rows {
                let (id, a, b) = row?;
                if let Some(f) = out.get_mut(&id) {
                    apply(f, a.max(0) as u32, b.max(0) as u32);
                }
            }
            Ok(())
        };
        fill(
            "SELECT project_id, COUNT(*), 0 FROM research_runs WHERE project_id IS NOT NULL GROUP BY project_id",
            &|f, n, _| f.research_runs = n,
        )?;
        fill(
            "SELECT project_id, COUNT(*), 0 FROM image_boards WHERE project_id IS NOT NULL AND deleted_at IS NULL GROUP BY project_id",
            &|f, n, _| f.boards = n,
        )?;
        fill(
            "SELECT b.project_id, COUNT(*), COALESCE(SUM(v.adopted), 0)
             FROM image_versions v JOIN image_boards b ON b.id = v.board_id
             WHERE b.project_id IS NOT NULL AND b.deleted_at IS NULL AND v.deleted_at IS NULL GROUP BY b.project_id",
            &|f, n, adopted| {
                f.images = n;
                f.adopted_images = adopted;
            },
        )?;
        fill(
            "SELECT project_id, COUNT(*), COALESCE(SUM(current_version_id IS NOT NULL), 0)
             FROM cad_designs WHERE project_id IS NOT NULL AND deleted_at IS NULL GROUP BY project_id",
            &|f, n, built| {
                f.designs = n;
                f.models = built;
            },
        )?;
        // 打样与定价(M4):成功的打样次数、成本模型里定没定价
        fill(
            "SELECT project_id, COALESCE(SUM(result = 'success'), 0), 0 FROM print_runs WHERE deleted_at IS NULL GROUP BY project_id",
            &|f, n, _| f.print_successes = n,
        )?;
        fill(
            "SELECT project_id, (chosen_price IS NOT NULL AND chosen_price > 0), 0 FROM cost_models",
            &|f, priced, _| f.has_price = priced > 0,
        )?;
        Ok(out)
    }

    /// 一个项目的产出计数。
    pub fn project_facts(&self, project_id: &str) -> DbResult<ProjectFacts> {
        // 项目不存在要报出来,而不是返回一组 0
        self.get_project(project_id)?;
        Ok(self.all_project_facts()?.remove(project_id).unwrap_or_default())
    }

    /// 项目中枢要的全部东西。
    pub fn project_overview(&self, project_id: &str) -> DbResult<ProjectOverview> {
        let facts = self.project_facts(project_id)?;
        let images = {
            let conn = self.r();
            let mut stmt = conn.prepare(
                "SELECT v.id, v.board_id, v.asset_id, v.adopted
                 FROM image_versions v JOIN image_boards b ON b.id = v.board_id
                 WHERE b.project_id = ?1 AND b.deleted_at IS NULL AND v.deleted_at IS NULL
                 ORDER BY v.adopted DESC, v.created_at DESC, v.id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![project_id, OVERVIEW_IMAGES], |r| {
                Ok(ProjectImage {
                    version_id: r.get(0)?,
                    board_id: r.get(1)?,
                    asset_id: r.get(2)?,
                    adopted: r.get(3)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let cost_fen: i64 = self.r().query_row(
            "SELECT COALESCE(SUM(amount), 0) FROM cost_entries WHERE project_id = ?1",
            [project_id],
            |r| r.get(0),
        )?;
        Ok(ProjectOverview {
            facts,
            research: self.list_research_of_project(project_id)?,
            boards: self.list_boards_of_project(project_id)?,
            images,
            designs: self.list_designs_of_project(project_id)?,
            cost_fen,
        })
    }
}

#[cfg(test)]
mod tests {
    use pp_common::gate::{gate_passed, needs_reason};
    use pp_common::imagery::ImagePurpose;
    use pp_common::{AssetKind, NewProject, Stage};

    use crate::testutil::TempDb;
    use crate::{new_id, DbError, NewAsset, NewImageVersion};

    fn project(t: &TempDb, title: &str, hypothesis: &str) -> String {
        t.create_project(&NewProject {
            title: title.into(),
            category: String::new(),
            hypothesis: hypothesis.into(),
        })
        .unwrap()
        .id
    }

    fn image(t: &TempDb, board: &str) -> String {
        let a = NewAsset::new(AssetKind::Image, "studio_image", &format!("assets/x/{}.jpg", new_id()), "jpg", 1);
        let asset_id = t.insert_asset(&a).unwrap().id;
        t.insert_image_version(&NewImageVersion {
            board_id: board.into(),
            parent_id: None,
            asset_id,
            prompt: "白底".into(),
            mode: "generate".into(),
            provider: "demo".into(),
            model: "demo".into(),
            aspect: Default::default(),
        })
        .unwrap()
        .id
    }

    #[test]
    fn a_new_project_has_nothing_and_each_workbench_adds_its_own_part() {
        let t = TempDb::new();
        let p = project(&t, "线缆夹", "   ");
        let other = project(&t, "收纳盒", "租房党想要免打孔的收纳");

        let f = t.project_facts(&p).unwrap();
        assert_eq!(f, Default::default(), "空白的假设不算写了假设");
        assert!(t.project_facts(&other).unwrap().has_hypothesis);

        // 图片:出了两张,采用一张;另一个项目、没挂项目的画板、拿掉的图都不算
        let board = t.create_board("线缆夹白底", ImagePurpose::ModelRef, Some(&p)).unwrap();
        let (first, second) = (image(&t, &board.id), image(&t, &board.id));
        t.set_image_adopted(&first, true).unwrap();
        let loose = t.create_board("随手画", ImagePurpose::Free, None).unwrap();
        image(&t, &loose.id);
        let f = t.project_facts(&p).unwrap();
        assert_eq!((f.boards, f.images, f.adopted_images), (1, 2, 1));
        assert!(gate_passed(Stage::Concept, &f));
        t.remove_image_version(&first).unwrap();
        let f = t.project_facts(&p).unwrap();
        assert_eq!((f.images, f.adopted_images), (1, 0), "拿掉的图不再算数");
        assert!(!gate_passed(Stage::Concept, &f));
        t.set_image_adopted(&second, true).unwrap();

        // 建模:只有设计还不算有模型
        let d = t.create_design("线缆夹", Some(&p)).unwrap();
        let f = t.project_facts(&p).unwrap();
        assert_eq!((f.designs, f.models), (1, 0));
        assert!(needs_reason(Stage::Model, Stage::Prototype, &f));
        t.delete_design(&d.id).unwrap();
        assert_eq!(t.project_facts(&p).unwrap().designs, 0, "删掉的设计不算");

        let all = t.all_project_facts().unwrap();
        assert_eq!(all.len(), 2, "没有产出的项目也在表里");
        assert_eq!(all[&other].boards, 0);
    }

    #[test]
    fn the_overview_lists_what_belongs_to_the_project_adopted_images_first() {
        let t = TempDb::new();
        let p = project(&t, "线缆夹", "桌面党会为理线付 19 元");
        let board = t.create_board("线缆夹白底", ImagePurpose::ModelRef, Some(&p)).unwrap();
        let older = image(&t, &board.id);
        std::thread::sleep(std::time::Duration::from_millis(3));
        let newer = image(&t, &board.id);
        t.set_image_adopted(&older, true).unwrap();
        t.create_design("线缆夹", Some(&p)).unwrap();
        t.create_design("别的零件", None).unwrap();
        t.add_cost(Some(&p), "image", 5.2, "出图").unwrap();
        t.add_cost(None, "image", 99.0, "没挂项目的花费").unwrap();

        let o = t.project_overview(&p).unwrap();
        assert_eq!(o.boards.len(), 1);
        assert_eq!(o.designs.len(), 1);
        assert_eq!(o.images.iter().map(|i| i.version_id.as_str()).collect::<Vec<_>>(), [older.as_str(), newer.as_str()], "采用的排前面");
        assert!(o.images[0].adopted && o.images[0].board_id == board.id);
        assert_eq!(o.cost_fen, 6, "只算挂在这个项目上的花费(向上取整)");
        assert!(o.research.is_empty());

        assert!(matches!(t.project_overview("no-such-project"), Err(DbError::NotFound(_))));
    }
}
