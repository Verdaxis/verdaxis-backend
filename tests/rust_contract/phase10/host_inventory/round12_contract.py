"""Strict mutation classification rejects setup errors and unrelated failures."""
import unittest

from boundary_mutations import classify, manifest_sites, scanned_sites


class Round12Contract(unittest.TestCase):
    def test_inventory_matches_source(self):
        self.assertEqual(len(scanned_sites()),31)
        self.assertEqual(scanned_sites(),manifest_sites())

    def test_strict_classifier(self):
        baseline={'ran':1,'success':True}
        failure={'kind':'failure','assertion':True,'designated':True,'first_line':'expected differs'}
        self.assertEqual(classify(baseline,{'ran':1,'reached':True,'success':False,'details':[failure]}),'MUTANT_KILLED')
        for update in ({'ran':0},{'reached':False},{'details':[dict(failure,kind='error')]},
                       {'details':[dict(failure,assertion=False)]},{'details':[dict(failure,designated=False)]}):
            with self.subTest(update=update):
                mutant={'ran':1,'reached':True,'success':False,'details':[failure]};mutant.update(update)
                self.assertEqual(classify(baseline,mutant),'INVALID')
        self.assertEqual(classify({'ran':1,'success':False},{'ran':1,'reached':True,'success':False,'details':[failure]}),'INVALID')
        self.assertEqual(classify(baseline,{'ran':1,'reached':True,'success':True,'details':[]}),'SURVIVED')


if __name__=='__main__':unittest.main(verbosity=2)
