//! Robot `.str` reader on small inline samples (an integration test: the antivirus on the
//! owner's PC blocks the lib unit-test exe).

use femcraft_import::robot::parse;

    const SAMPLE: &str = "ROBOT97\nSHEll\nNODes 3  ELEments 3\nUNIts\nLENgth=m    FORces=N\nUNIts END\n\
NODes\n' x\n 1 0,0E+0 0,0E+0 0,0E+0\n 2 5,0E+0 0,0E+0 0,0E+0\n 3 5,0E+0 5,0E+0 0,0E+0\n 4 0,0E+0 5,0E+0 0,0E+0\n 5 0 0 3\n\
SUPports\n' Label        - free\n1\n2 RY RX\nELEments\nBARs 1\n' h\n 1 1 5\nQUAdrangular 4\n 2 1 2 3 4\nPROperties\n\
E=3,3E+10 NI=2,0E-1 RO=0,0E+0 LX=1,0E-5 G=1,375E+10 NAMe=\"C30\"\n1_\nAx=3,2E-1 Iy=3,4E-3 Iz=8,5E-4 Ix=1E-4 NAMe=\"400x800\"\n2 TO 2_\nTH=2,5E-1\n\
LOAds\nANAlysis LINear\nCASe #1 1 Dead\nNODes\n5 FZ=-3,0E+3 CX=1,0E+1\nELEments\n2 PZ=-5,0E+3\n1 PZ=-1,0E+2\n\
CASe #2 2 Wind\nELEments\n2 X=0 PZ=1 TILl X=1 PZ=2 RELative\n\
COMbination #1 1 ULS\n 1 1,35E+0\n 2 1,5E+0\nEND\n";

    #[test]
    fn reads_small_model() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.joints.len(), 5);
        assert_eq!(m.joints[&2], [5.0, 0.0, 0.0]);
        assert_eq!(m.supports[&1], [true; 6]);
        assert_eq!(m.supports[&2], [true, true, true, false, false, true]);
        assert_eq!(m.members[&1], (1, 5));
        assert_eq!(m.plates[&2], vec![1, 2, 3, 4]);
        assert_eq!(m.materials.len(), 1);
        assert_eq!(m.materials[0].name, "C30");
        let p = m.member_props[&1];
        assert!((p.zd - 0.4).abs() < 1e-12 && (p.yd - 0.8).abs() < 1e-12);
        assert_eq!(m.plate_thickness[&2], 0.25);
        assert_eq!(m.element_material[&2], "C30");
        assert_eq!(m.load_cases[&1], "Dead");
        assert_eq!(m.joint_loads[0].force, [0.0, 0.0, -3000.0]);
        assert_eq!(m.joint_loads[0].moment, [10.0, 0.0, 0.0]);
        assert_eq!(m.element_loads.len(), 1);
        assert_eq!(m.member_loads.len(), 1);
        assert_eq!(m.combination_defs[0].1, vec![(1, 1.35, 1.35), (2, 1.5, 1.5)]);
        assert_eq!(m.skipped.values().sum::<usize>(), 1);
    }

    #[test]
    fn native_support_labels_are_restrained_dofs() {
        // Robot's own files (no `Label - free` comment): labels name the restrained DOFs.
        let t = SAMPLE.replace("' Label        - free\n", "");
        let m = parse(&t).unwrap();
        assert_eq!(m.supports[&1], [true; 6]);
        assert_eq!(m.supports[&2], [false, false, false, true, true, false]);
    }

    #[test]
    fn rejects_other_text() {
        assert!(parse("STAAD SPACE\n").is_err());
    }
