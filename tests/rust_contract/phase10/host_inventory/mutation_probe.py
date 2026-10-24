"""Structured single-method unittest probe for boundary mutations."""
import importlib.util
import json
import sys
import traceback
import unittest
from pathlib import Path


def main():
    test_path, method, source, line, output = sys.argv[1:]
    reached=[]
    target=Path(source).resolve()
    def trace(frame,event,arg):
        if event=='line' and frame.f_lineno==int(line) and Path(frame.f_code.co_filename).resolve()==target:
            reached.append(True)
        return trace
    try:
        spec=importlib.util.spec_from_file_location('designated_mutation_test',test_path)
        module=importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        suite=unittest.defaultTestLoader.loadTestsFromName(method,module)
        class Result(unittest.TestResult):
            def __init__(self):
                super().__init__();self.details=[]
            def save(self,test,err,kind):
                # Keep only structured assertion diagnostic, never raw child output.
                assertion=isinstance(err[1],AssertionError)
                frames=traceback.extract_tb(err[2])
                designated=any(f.name==method.split('.')[-1] for f in frames)
                self.details.append({'test':test.id(),'kind':kind,'assertion':assertion,
                                     'designated':designated,'first_line':str(err[1]).splitlines()[0][:300] if str(err[1]).splitlines() else type(err[1]).__name__})
            def addFailure(self,test,err):
                super().addFailure(test,err);self.save(test,err,'failure')
            def addError(self,test,err):
                super().addError(test,err);self.save(test,err,'error')
            def addSubTest(self,test,subtest,err):
                super().addSubTest(test,subtest,err)
                if err:self.save(subtest,err,'failure' if issubclass(err[0],test.failureException) else 'error')
        result=Result()
        sys.settrace(trace)
        try:suite.run(result)
        finally:sys.settrace(None)
        document={'ran':result.testsRun,'reached':bool(reached),'success':result.wasSuccessful(),'details':result.details}
    except BaseException as error:
        document={'ran':0,'reached':bool(reached),'success':False,'details':[{'kind':'error','assertion':False,'designated':False,'first_line':type(error).__name__}]}
    Path(output).write_text(json.dumps(document))


if __name__=='__main__':main()
